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

async fn disconnect(client: &mut Client, reason: &str) {
    let json = serde_json::to_string(&serde_json::json!({ "text": reason })).unwrap();
    let _ = LoginDisconnect { reason: json.as_str() }.write_packet(&mut client.stream).await;
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
    let Some(expected_token) = client.ctx.verify_token else {
        disconnect(client, "Unexpected encryption response").await;
        return;
    };
    let Some(username) = client.ctx.username.clone() else {
        disconnect(client, "Missing login start").await;
        return;
    };

    let secret = match decrypt_rsa(p.shared_secret, &client.server.keys.private) {
        Ok(secret) => secret,
        Err(_) => {
            disconnect(client, "Failed to decrypt shared secret").await;
            return;
        }
    };
    let token = match decrypt_rsa(p.verify_token, &client.server.keys.private) {
        Ok(token) => token,
        Err(_) => {
            disconnect(client, "Failed to decrypt verify token").await;
            return;
        }
    };

    if token != expected_token {
        disconnect(client, "Verify tokens do not match").await;
        return;
    }
    if secret.len() != 16 {
        disconnect(client, "Invalid shared secret length").await;
        return;
    }
    let shared: [u8; 16] = secret[..16].try_into().unwrap();

    let hash = server_hash(SERVER_ID, &shared, &client.server.keys.public);

    let profile: Response = match authenticate(&username, &hash).await {
        Ok(profile) => profile,
        Err(_) => {
            disconnect(client, "Failed to verify username!").await;
            return;
        }
    };

    if profile.name != username {
        disconnect(client, "Username mismatch").await;
        return;
    }

    let game_profile = match to_game_profile(&profile, &username) {
        Ok(game_profile) => game_profile,
        Err(_) => {
            disconnect(client, "Invalid profile id").await;
            return;
        }
    };
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
}

async fn handle_login_acknowledge(client: &mut Client) {
    client.ctx.state = mc_protocol::State::Config;
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
