use std::time::Instant;

use mc_protocol::packets::Packets;
use mc_protocol::packets::client::{
    GameEvent, PlayerAbilities, SetDefaultSpawnPosition, SetEntityMetadata, SetHealth,
};
use mc_protocol::packets::server::PlayerAction as PlayerActionPacket;

use crate::Client;
use crate::error::Error;
use crate::inventory::InventoryAction;

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
                client
                    .session
                    .as_ref()
                    .ok_or(Error::UnexpectedResponse)?
                    .confirm_teleport(p.teleport_id);
            }
        }
        Packets::SetPlayerPosition(p) => {
            if !client.session.as_ref().is_some_and(|session| session.ready()) {
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
            );
        }
        Packets::SetPlayerPositionRotation(p) => {
            if !client.session.as_ref().is_some_and(|session| session.ready()) {
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
            );
        }
        Packets::SetPlayerRotation(p) => {
            if !client.session.as_ref().is_some_and(|session| session.ready()) {
                return Ok(());
            }
            if !p.yaw.is_finite() || !p.pitch.is_finite() {
                return Err(Error::InvalidPlayerData);
            }
            client.proposed.yaw = p.yaw;
            client.proposed.pitch = p.pitch;
            client.proposed.on_ground = p.flags & 0x01 != 0;
            client.propose_position();
        }
        Packets::SetPlayerMovementFlags(p) => {
            if client.session.as_ref().is_some_and(|session| session.ready()) {
                client.proposed.on_ground = p.flags & 0x01 != 0;
                client.propose_position();
            }
        }
        Packets::PlayerInput(p) => {
            let input = crate::movement::MovementInput::from_flags(
                p.flags,
                client.proposed.yaw,
                client.proposed.pitch,
            );
            let sneaking = input.sneaking;
            if client.sneaking != sneaking {
                client.sneaking = sneaking;
                client.propose_position();
            }
        }
        Packets::PlayerCommand(p) => {
            if p.entity_id != client.entity_id()? {
                return Ok(());
            }
            if p.action == 1 || p.action == 2 {
                client.sprinting = p.action == 1;
                client.propose_position();
            }
        }
        Packets::ClickContainer(p) => {
            if let Some(session) = &client.session {
                session.inventory(InventoryAction::from_click(
                    p.window_id,
                    p.state_id,
                    p.slot,
                    p.button,
                    p.mode,
                    p.predicted_slots,
                ));
            }
        }
        Packets::CloseContainer(p) => {
            if let Some(session) = &client.session {
                session.inventory(InventoryAction::Close { window_id: p.window_id });
            }
        }
        Packets::SetHeldItem(p) => {
            if let Some(session) = &client.session {
                session.inventory(InventoryAction::from_selection(p.slot));
            }
        }
        Packets::PlayerAction(p) => {
            if let Some(session) = &client.session {
                match p.action {
                    PlayerActionPacket::DROP_ALL_ITEMS | PlayerActionPacket::DROP_ITEM => {
                        session.inventory(InventoryAction::RejectDrop)
                    }
                    PlayerActionPacket::SWAP_ITEM_WITH_OFFHAND => {
                        session.inventory(InventoryAction::SwapHands)
                    }
                    _ => {}
                }
            }
            // Disabled digging still has client-side predictions. Acknowledge
            // their sequence and restore authority rather than leaving ghost air.
            if matches!(p.action, 0..=3) {
                client.resync_blocks([crate::arena::unpack(p.location)]).await?;
                client
                    .send(&mc_protocol::packets::client::AcknowledgeBlockChange {
                        sequence_id: p.sequence,
                    })
                    .await?;
            }
        }
        Packets::UseItemOn(p) => {
            if !(0..=1).contains(&p.hand)
                || !(0..=5).contains(&p.face)
                || ![p.cursor_x, p.cursor_y, p.cursor_z].into_iter().all(f32::is_finite)
            {
                return Err(Error::InvalidPlayerData);
            }
            let [x, y, z] = crate::arena::unpack(p.location);
            let used = crate::interaction::BlockUse {
                sequence: p.sequence,
                pos: [x, y, z],
                cursor: [p.cursor_x, p.cursor_y, p.cursor_z],
                main_hand: p.hand == 0,
            };
            if client.session.as_ref().is_some_and(|session| session.use_block(used)) {
                return Ok(());
            }
            client
                .resync_blocks(crate::world_sync::resync_cells(crate::lobby::Transform {
                    x: f64::from(x) + 0.5,
                    y: f64::from(y),
                    z: f64::from(z) + 0.5,
                    ..Default::default()
                }))
                .await?;
            client
                .send(&mc_protocol::packets::client::AcknowledgeBlockChange {
                    sequence_id: p.sequence,
                })
                .await?;
            if let Some(session) = &client.session {
                session.inventory(InventoryAction::Resync);
            }
        }
        Packets::UseItem(p) => {
            if !(0..=1).contains(&p.hand) || !p.yaw.is_finite() || !p.pitch.is_finite() {
                return Err(Error::InvalidPlayerData);
            }
            client
                .send(&mc_protocol::packets::client::AcknowledgeBlockChange {
                    sequence_id: p.sequence,
                })
                .await?;
            if let Some(session) = &client.session {
                session.inventory(InventoryAction::Resync);
            }
        }
        Packets::ClientCommand(p) => {
            if p.action == 0
                && let Some(session) = &client.session
            {
                session.respawn();
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

/// Preserve packet order; only the world owner can accept a movement proposal.
fn move_player(
    client: &mut Client,
    x: f64,
    y: f64,
    z: f64,
    yaw: Option<f32>,
    pitch: Option<f32>,
    on_ground: bool,
) {
    client.proposed.x = x;
    client.proposed.y = y;
    client.proposed.z = z;
    if let Some(yaw) = yaw {
        client.proposed.yaw = yaw;
    }
    if let Some(pitch) = pitch {
        client.proposed.pitch = pitch;
    }
    client.proposed.on_ground = on_ground;
    client.propose_position();
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
    let crate::lobby::Transform { x: sx, y: sy, z: sz, yaw, pitch, .. } = client.pos;
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

    client.chunks.reconcile(&client.server.lobby, (scx, scz));
    let session = client.session.as_ref().ok_or(Error::UnexpectedResponse)?;
    session.teleport(client.pos);
    session.inventory(InventoryAction::Resync);

    Ok(())
}
