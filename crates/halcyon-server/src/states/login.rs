use mc_protocol::auth::Response;
use mc_protocol::error::DecodeError;
use mc_protocol::packets::Packets;
use mc_protocol::packets::client::*;
use mc_protocol::packets::server::*;
use mc_protocol::types::{GameProfile, ProfileProperty};
use uuid::Uuid;

use crate::{Client, auth::authenticate, encryption::*, error::Error, lifecycle::LoginProgress};

const SERVER_ID: &str = "";

async fn handle_login_start(p: LoginStart<'_>, client: &mut Client) -> Result<(), Error> {
    if p.name.is_empty()
        || p.name.len() > 16
        || !p.name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(Error::InvalidPlayerData);
    }
    client.login.advance(LoginProgress::Start, LoginProgress::Encryption)?;
    client.ctx.username = Some(p.name.to_owned());
    let verify_token = generate_verify_token()?;
    let server = std::sync::Arc::clone(&client.server);
    client
        .send(&EncryptionRequest {
            server_id: SERVER_ID,
            public_key: &server.keys.public,
            verify_token: &verify_token,
            should_authenticate: true,
        })
        .await?;
    client.ctx.verify_token = Some(verify_token);
    Ok(())
}

fn parse_profile_id(raw: &str) -> Result<Uuid, DecodeError> {
    if raw.len() != 32 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DecodeError::InvalidProfileId);
    }
    Uuid::parse_str(raw).map_err(|_| DecodeError::InvalidProfileId)
}

fn to_game_profile<'p>(
    res: &'p Response,
    username: &'p str,
) -> Result<GameProfile<'p>, DecodeError> {
    Ok(GameProfile {
        uuid: parse_profile_id(&res.id)?,
        username,
        properties: res
            .properties
            .iter()
            .map(|prop| ProfileProperty {
                name: &prop.name,
                value: &prop.value,
                signature: prop.signature.as_deref(),
            })
            .collect(),
    })
}

async fn handle_encryption_response(
    p: EncryptionResponse<'_>,
    client: &mut Client,
) -> Result<(), Error> {
    let result = login(p, client).await;
    if let Err(error) = &result
        && let Some(reason) = error.disconnect_reason()
    {
        let text = serde_json::to_string(&serde_json::json!({ "text": reason }))?;
        let _ = client.send(&LoginDisconnect { reason: &text }).await;
    }
    result
}

async fn login(p: EncryptionResponse<'_>, client: &mut Client) -> Result<(), Error> {
    if client.login != LoginProgress::Encryption {
        return Err(Error::UnexpectedResponse);
    }
    let expected_token = client.ctx.verify_token.take().ok_or(Error::UnexpectedResponse)?;
    let username = client.ctx.username.clone().ok_or(Error::MissingStart)?;
    let secret = decrypt_rsa(p.shared_secret, &client.server.keys.private)
        .map_err(|_| Error::DecryptSecret)?;
    let token = decrypt_rsa(p.verify_token, &client.server.keys.private)
        .map_err(|_| Error::DecryptToken)?;
    if token != expected_token {
        return Err(Error::TokenMismatch);
    }
    let shared: [u8; 16] = secret.as_slice().try_into().map_err(|_| Error::BadSecretLen)?;
    // All subsequent traffic, including authentication failures, is encrypted.
    let (enc, dec) = ciphers(&shared)?;
    client.stream.enable_encryption(enc, dec)?;
    let hash = server_hash(SERVER_ID, &shared, &client.server.keys.public);
    let profile: Response =
        authenticate(&username, &hash).await.map_err(|_| Error::VerifyFailed)?;
    if profile.name != username {
        return Err(Error::NameMismatch);
    }
    let game_profile = to_game_profile(&profile, &username).map_err(|_| Error::BadProfileId)?;
    let player_uuid = game_profile.uuid;
    client.send(&LoginSuccess { profile: game_profile, session_id: Uuid::new_v4() }).await?;
    client.ctx.player_uuid = Some(player_uuid);
    client.ctx.profile_properties = profile.properties;
    client.login.advance(LoginProgress::Encryption, LoginProgress::Acknowledge)?;
    Ok(())
}

async fn handle_login_acknowledge(client: &mut Client) -> Result<(), Error> {
    client.login.advance(LoginProgress::Acknowledge, LoginProgress::Complete)?;
    client.ctx.state = mc_protocol::State::Config;
    let server = std::sync::Arc::clone(&client.server);
    for frame in &server.packets.configuration {
        client.send_encoded(frame).await?;
        glommio::executor().yield_now().await;
    }
    Ok(())
}

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::LoginStart(p) => handle_login_start(p, client).await,
        Packets::EncryptionResponse(p) => handle_encryption_response(p, client).await,
        Packets::LoginAcknowledge(_) => handle_login_acknowledge(client).await,
        Packets::LoginPluginResponse(_) | Packets::LoginCookieResponse(_) => Ok(()),
        _ => Err(Error::UnexpectedResponse),
    }
}
