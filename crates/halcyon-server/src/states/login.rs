use mc_protocol::auth::Response;
use mc_protocol::error::DecodeError;
use mc_protocol::packets::Packets;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::*;
use mc_protocol::packets::server::*;
use mc_protocol::types::{GameProfile, ProfileProperty};
use uuid::Uuid;

use crate::Client;
use crate::auth::authenticate;
use crate::encryption::*;

// NOTE: we should probably separate client into different states
// only an unauthenticated client needs the verify token
// while authenticated client will need to store more data

const SERVER_ID: &str = "";

/// Login failures. Only auth failures justify telling the player why
/// (vanilla "Failed to verify username!"); protocol violations and
/// server-side errors close silently, details stay server-side.
#[derive(Debug)]
enum LoginError {
    UnexpectedResponse,
    MissingStart,
    DecryptSecret,
    DecryptToken,
    TokenMismatch,
    BadSecretLen,
    VerifyFailed,
    NameMismatch,
    BadProfileId,
}

impl LoginError {
    /// Player-facing reason, if the failure justifies a `LoginDisconnect`.
    fn disconnect_reason(&self) -> Option<&'static str> {
        match self {
            LoginError::VerifyFailed | LoginError::NameMismatch => {
                Some("Failed to verify username!")
            }
            _ => None,
        }
    }
}

async fn disconnect(client: &mut Client, reason: &str) {
    let json = serde_json::to_string(&serde_json::json!({ "text": reason })).unwrap();
    let _ = LoginDisconnect { reason: json.as_str() }.write_packet(&mut client.stream).await;
    close(client).await;
}

async fn close(client: &mut Client) {
    let _ = futures_lite::AsyncWriteExt::close(&mut client.stream).await;
}

async fn handle_login_start(p: LoginStart<'_>, client: &mut Client) {
    client.ctx.username = Some(p.name.to_owned());
    client.ctx.player_uuid = Some(p.uuid);

    let verify_token = generate_verify_token().unwrap();

    let request = EncryptionRequest {
        server_id: SERVER_ID,
        public_key: &client.server.keys.public,
        verify_token: &verify_token,
        should_authenticate: true,
    };

    request.write_packet(&mut client.stream).await.unwrap();

    client.ctx.verify_token = Some(verify_token);
}

fn parse_profile_id(raw: &str) -> Result<Uuid, DecodeError> {
    if raw.len() != 32 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DecodeError::InvalidProfileId);
    }

    let mut hex = String::with_capacity(36);
    hex.push_str(&raw[0..8]);
    hex.push('-');
    hex.push_str(&raw[8..12]);
    hex.push('-');
    hex.push_str(&raw[12..16]);
    hex.push('-');
    hex.push_str(&raw[16..20]);
    hex.push('-');
    hex.push_str(&raw[20..32]);

    Uuid::parse_str(&hex).map_err(|_| DecodeError::InvalidProfileId)
}

fn to_game_profile<'p>(
    res: &'p Response,
    username: &'p str,
) -> Result<GameProfile<'p>, DecodeError> {
    let uuid = parse_profile_id(&res.id)?;
    let mut properties = Vec::with_capacity(res.properties.len());

    for prop in &res.properties {
        properties.push(ProfileProperty {
            name: prop.name.as_str(),
            value: prop.value.as_str(),
            signature: prop.signature.as_deref(),
        });
    }

    Ok(GameProfile { uuid, username, properties })
}

async fn handle_encryption_response(p: EncryptionResponse<'_>, client: &mut Client) {
    if let Err(e) = login(p, client).await {
        eprintln!("login failed: {e:?}");
        match e.disconnect_reason() {
            Some(reason) => disconnect(client, reason).await,
            None => close(client).await,
        }
    }
}

async fn login(p: EncryptionResponse<'_>, client: &mut Client) -> Result<(), LoginError> {
    let Some(expected_token) = client.ctx.verify_token else {
        Err(LoginError::UnexpectedResponse)?
    };
    let Some(username) = client.ctx.username.clone() else {
        Err(LoginError::MissingStart)?
    };

    let secret = decrypt_rsa(p.shared_secret, &client.server.keys.private)
        .map_err(|_| LoginError::DecryptSecret)?;
    let token = decrypt_rsa(p.verify_token, &client.server.keys.private)
        .map_err(|_| LoginError::DecryptToken)?;

    if token != expected_token {
        Err(LoginError::TokenMismatch)?;
    }
    if secret.len() != 16 {
        Err(LoginError::BadSecretLen)?;
    }
    let shared: [u8; 16] = secret[..16].try_into().unwrap();

    let hash = server_hash(SERVER_ID, &shared, &client.server.keys.public);
    let profile: Response =
        authenticate(&username, &hash).await.map_err(|_| LoginError::VerifyFailed)?;

    if profile.name != username {
        Err(LoginError::NameMismatch)?;
    }

    let game_profile =
        to_game_profile(&profile, &username).map_err(|_| LoginError::BadProfileId)?;
    // Session ID is a fresh random UUID identifying this login session
    // (Velocity: `UUID.randomUUID()`; BungeeCord: same since MINECRAFT_26_2).
    let session_id = Uuid::new_v4();
    let player_uuid = game_profile.uuid;
    let success = LoginSuccess { profile: game_profile, session_id };

    let (enc, dec) = ciphers(&shared).unwrap();
    client.enable_encryption(enc, dec);
    success.write_packet(&mut client.stream).await.unwrap();

    client.ctx.verify_token = None;
    client.ctx.player_uuid = Some(player_uuid);

    Ok(())
}

async fn handle_login_acknowledge(client: &mut Client) {
    use mc_protocol::data::registries;
    use mc_protocol::types::{KnownPack, RegistryEntry, Tag, TaggedRegistry};

    client.ctx.state = mc_protocol::State::Config;

    KnownPacks { packs: vec![KnownPack { namespace: "minecraft", id: "core", version: "26.3" }] }
        .write_packet(&mut client.stream)
        .await
        .unwrap();

    for (registry, entries) in registries::all() {
        RegistryData {
            registry,
            entries: entries
                .into_iter()
                .map(|(id, data)| RegistryEntry { id, data: Some(data) })
                .collect(),
        }
        .write_packet(&mut client.stream)
        .await
        .unwrap();
    }

    FeatureFlags { flags: vec!["minecraft:vanilla"] }
        .write_packet(&mut client.stream)
        .await
        .unwrap();

    let mut tagged: Vec<TaggedRegistry> = registries::all_tags()
        .into_iter()
        .map(|(registry, tags)| TaggedRegistry {
            registry,
            tags: tags
                .into_iter()
                .map(|(name, entries)| Tag {
                    name,
                    entries: entries.into_iter().map(mc_protocol::types::VarInt).collect(),
                })
                .collect(),
        })
        .collect();
    tagged.extend(registries::builtin_empty_tags().into_iter().map(|(registry, tags)| {
        TaggedRegistry {
            registry,
            tags: tags.into_iter().map(|name| Tag { name, entries: vec![] }).collect(),
        }
    }));
    UpdateTags { registries: tagged }.write_packet(&mut client.stream).await.unwrap();

    FinishConfiguration.write_packet(&mut client.stream).await.unwrap();
}

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    use Packets::*;

    match pkt {
        LoginStart(p) => handle_login_start(p, client).await,
        EncryptionResponse(p) => handle_encryption_response(p, client).await,
        LoginAcknowledge(_) => handle_login_acknowledge(client).await,
        LoginPluginResponse(_) | LoginCookieResponse(_) => {}
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}
