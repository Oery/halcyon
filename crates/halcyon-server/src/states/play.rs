use std::time::{Duration, Instant};

use mc_protocol::packets::Packets;
use mc_protocol::packets::client::{
    GameEvent, PlayerAbilities, SetDefaultSpawnPosition, SetEntityMetadata, SetHealth,
    SyncPlayerPosition,
};

use crate::Client;
use crate::error::Error;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::ConfirmTeleportation(p) => {
            if client.pending_teleport == Some(p.teleport_id) {
                if !p.x.is_finite()
                    || !p.y.is_finite()
                    || !p.z.is_finite()
                    || !p.yaw.is_finite()
                    || !p.pitch.is_finite()
                {
                    return Err(Error::InvalidPlayerData);
                }
                client.pending_teleport = None;
                client.teleport_deadline = None;
                // Confirmation acknowledges the server's spawn, rather than
                // providing a second client-selected initial position.
                client.session.as_ref().ok_or(Error::UnexpectedResponse)?.activate();
            }
        }
        Packets::SetPlayerPosition(p) => {
            if client.pending_teleport.is_some() {
                return Ok(());
            }
            if !p.x.is_finite() || !p.feet_y.is_finite() || !p.z.is_finite() {
                return Err(Error::InvalidPlayerData);
            }
            move_player(
                client,
                p.x.clamp(-30_000_000.0, 30_000_000.0),
                p.feet_y.clamp(-20_000_000.0, 20_000_000.0),
                p.z.clamp(-30_000_000.0, 30_000_000.0),
                None,
                None,
                p.flags & 0x01 != 0,
            )
            .await?;
        }
        Packets::SetPlayerPositionRotation(p) => {
            if client.pending_teleport.is_some() {
                return Ok(());
            }
            if !p.x.is_finite()
                || !p.feet_y.is_finite()
                || !p.z.is_finite()
                || !p.yaw.is_finite()
                || !p.pitch.is_finite()
            {
                return Err(Error::InvalidPlayerData);
            }
            move_player(
                client,
                p.x.clamp(-30_000_000.0, 30_000_000.0),
                p.feet_y.clamp(-20_000_000.0, 20_000_000.0),
                p.z.clamp(-30_000_000.0, 30_000_000.0),
                Some(p.yaw),
                Some(p.pitch),
                p.flags & 0x01 != 0,
            )
            .await?;
        }
        Packets::SetPlayerRotation(p) => {
            if client.pending_teleport.is_some() {
                return Ok(());
            }
            if !p.yaw.is_finite() || !p.pitch.is_finite() {
                return Err(Error::InvalidPlayerData);
            }
            client.pos.yaw = p.yaw;
            client.pos.pitch = p.pitch;
            client.pos.on_ground = p.flags & 0x01 != 0;
            client.publish_position();
        }
        Packets::SetPlayerMovementFlags(p) => {
            if client.pending_teleport.is_none() {
                client.pos.on_ground = p.flags & 0x01 != 0;
                client.publish_position();
            }
        }
        Packets::PlayClientInformation(p) => {
            client.ctx.skin_parts = p.skin_parts.0;
            if let Some(session) = &client.session {
                session.publish_skin_parts(p.skin_parts.0);
            }
            client.chunks.set_view_distance(p.render_distance);
            client.chunks.reconcile(
                &client.server.lobby,
                ((client.pos.x.floor() as i32) >> 4, (client.pos.z.floor() as i32) >> 4),
            );
            let entity_id = client.entity_id()?;
            client.send(&SetEntityMetadata { entity_id, skin_parts: p.skin_parts.0 }).await?;
        }
        Packets::PlayKeepAliveResponse(p) => client.keepalive.acknowledge(p.id)?,
        Packets::ChunkBatchReceived(p) => {
            client.chunks.acknowledge(p.chunks_per_tick, Instant::now())?
        }
        Packets::ClientTickEnd(_) | Packets::PlayerLoaded(_) => {}
        _ => {}
    };

    Ok(())
}

/// Burst sent once on Play entry: player state, center chunk, chunk batch,
/// teleport, first keepalive.
fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x & 0x3FFFFFF) as i64) << 38) | (((z & 0x3FFFFFF) as i64) << 12) | ((y & 0xFFF) as i64)
}

/// Apply a validated move; queue chunks when the chunk column changes.
async fn move_player(
    client: &mut Client,
    x: f64,
    y: f64,
    z: f64,
    yaw: Option<f32>,
    pitch: Option<f32>,
    on_ground: bool,
) -> Result<(), Error> {
    let old_cx = (client.pos.x.floor() as i32) >> 4;
    let old_cz = (client.pos.z.floor() as i32) >> 4;
    client.pos.x = x;
    client.pos.y = y;
    client.pos.z = z;
    if let Some(yaw) = yaw {
        client.pos.yaw = yaw;
    }
    if let Some(pitch) = pitch {
        client.pos.pitch = pitch;
    }
    client.pos.on_ground = on_ground;
    let cx = (x.floor() as i32) >> 4;
    let cz = (z.floor() as i32) >> 4;
    if (cx, cz) != (old_cx, old_cz) {
        client.chunks.reconcile(&client.server.lobby, (cx, cz));
    }
    client.publish_position();
    Ok(())
}
/// Sends the two packets the client renders the player's skin from:
/// `Player Info Update` (Mojang `textures` property verified at login)
/// and `Set Entity Metadata` (skin-parts bitmask from `Client Information`).
async fn send_skin(client: &mut Client) -> Result<(), Error> {
    let skin_parts = client.ctx.skin_parts;
    let info = client.profile_info.clone().ok_or(Error::UnexpectedResponse)?;
    client.send_encoded(&info).await?;
    // Entity ID must match the one sent in `PlayLogin`.
    let entity_id = client.entity_id()?;
    client.send(&SetEntityMetadata { entity_id, skin_parts }).await?;

    Ok(())
}
pub async fn enter_play(client: &mut Client) -> Result<(), Error> {
    send_skin(client).await?;
    let (sx, sy, sz, yaw, pitch) = client.server.lobby.spawn;
    let scx = (sx.floor() as i32) >> 4;
    let scz = (sz.floor() as i32) >> 4;
    client.send(&GameEvent { event: 13, value: 0.0 }).await?;
    client
        .send(&SetDefaultSpawnPosition {
            dimension_name: "minecraft:overworld",
            location: pack_block_pos(sx.floor() as i32, sy.floor() as i32, sz.floor() as i32),
            yaw,
            pitch,
        })
        .await?;
    client.send(&PlayerAbilities { flags: 0x00, flying_speed: 0.05, fov_modifier: 0.1 }).await?;
    client.send(&SetHealth { health: 20.0, food: 20, saturation: 5.0 }).await?;

    client.pos.x = sx;
    client.pos.y = sy;
    client.pos.z = sz;
    client.pos.yaw = yaw;
    client.pos.pitch = pitch;
    client.chunks.reconcile(&client.server.lobby, (scx, scz));

    client
        .send(&SyncPlayerPosition {
            teleport_id: 1,
            x: sx,
            y: sy,
            z: sz,
            vel_x: 0.0,
            vel_y: 0.0,
            vel_z: 0.0,
            yaw,
            pitch,
            flags: 0,
        })
        .await?;
    client.next_teleport_id = 1;
    client.pending_teleport = Some(1);
    client.teleport_deadline = Some(Instant::now() + Duration::from_secs(30));

    Ok(())
}
