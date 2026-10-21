use futures_lite::AsyncWriteExt;
use std::time::Instant;

use mc_protocol::packets::client::{
    ChunkBatchFinished, GameEvent, PLAYER_ENTITY_ID, PlayKeepAlive, PlayerAbilities,
    PlayerInfoUpdate, SetCenterChunk, SetDefaultSpawnPosition, SetEntityMetadata, SetHealth,
    SyncPlayerPosition, UnloadChunk,
};
use mc_protocol::packets::{Packets, Payload};
use mc_protocol::types::ProfileProperty;

use crate::Client;
use crate::error::Error;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::ConfirmTeleportation(p) => {
            if client.pending_teleport == Some(p.teleport_id) {
                client.pending_teleport = None;
                client.pos.x = p.x;
                client.pos.y = p.y;
                client.pos.z = p.z;
                client.pos.yaw = p.yaw;
                client.pos.pitch = p.pitch;
            }
        }
        Packets::SetPlayerPosition(p) => {
            if client.pending_teleport.is_some() {
                return Ok(());
            }
            if !p.x.is_finite() || !p.feet_y.is_finite() || !p.z.is_finite() {
                eprintln!("non-finite SetPlayerPosition coordinates, closing connection");
                let _ = client.stream.close().await;
                return Ok(());
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
                eprintln!("non-finite SetPlayerPositionRotation coordinates, closing connection");
                let _ = client.stream.close().await;
                return Ok(());
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
                eprintln!("non-finite SetPlayerRotation coordinates, closing connection");
                let _ = client.stream.close().await;
                return Ok(());
            }
            client.pos.yaw = p.yaw;
            client.pos.pitch = p.pitch;
            client.pos.on_ground = p.flags & 0x01 != 0;
        }
        Packets::PlayClientInformation(p) => {
            client.ctx.skin_parts = p.skin_parts.0;
            client
                .send(&SetEntityMetadata {
                    entity_id: PLAYER_ENTITY_ID,
                    skin_parts: p.skin_parts.0,
                })
                .await?;
        }
        Packets::PlayKeepAliveResponse(_)
        | Packets::ChunkBatchReceived(_)
        | Packets::ClientTickEnd(_)
        | Packets::PlayerLoaded(_) => {}
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };

    Ok(())
}

/// Burst sent once on Play entry: player state, center chunk, chunk batch,
/// teleport, first keepalive.
fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x & 0x3FFFFFF) as i64) << 38) | (((z & 0x3FFFFFF) as i64) << 12) | ((y & 0xFFF) as i64)
}

async fn send_chunk(
    client: &mut Client,
    chunk: &std::sync::Arc<halcyon_world::Chunk>,
) -> Result<(), Error> {
    let server = std::sync::Arc::clone(&client.server);
    let frame = server
        .packets
        .chunks
        .get(&(chunk.x, chunk.z))
        .ok_or_else(|| std::io::Error::other("missing cached chunk"))?;
    client.send_chunk_parts(&frame.header, &frame.data, &server.packets.chunk_suffix).await
}

async fn send_empty_chunk(client: &mut Client, x: i32, z: i32) -> Result<(), Error> {
    let server = std::sync::Arc::clone(&client.server);
    let cache = &server.packets;
    client.packet_buffer.clear();
    client.packet_buffer.extend_from_slice(&cache.empty.header);
    let offset = cache.empty_coordinates;
    client.packet_buffer[offset..offset + 4].copy_from_slice(&x.to_be_bytes());
    client.packet_buffer[offset + 4..offset + 8].copy_from_slice(&z.to_be_bytes());
    client.send_buffered_chunk(&cache.empty.data, &cache.chunk_suffix).await
}

/// Desired chunk set: view-distance square around `(cx, cz)`, intersected
/// with loaded chunks.
fn wanted_chunks(
    lobby: &halcyon_world::World,
    cx: i32,
    cz: i32,
    view_distance: i32,
) -> Vec<std::sync::Arc<halcyon_world::Chunk>> {
    let mut out = Vec::new();
    for dz in -view_distance..=view_distance {
        for dx in -view_distance..=view_distance {
            if let Some(chunk) = lobby.get(cx + dx, cz + dz) {
                out.push(std::sync::Arc::clone(chunk));
            }
        }
    }
    out.sort_by_key(|c| {
        let dx = c.x - cx;
        let dz = c.z - cz;
        dx * dx + dz * dz
    });
    out
}

/// Reconcile `sent_chunks` with the wanted set around the player's new
/// chunk, plus a 1-wide ring of empty padding so edge chunks have the
/// neighbors the client needs to mesh them. Unload far chunks first, then
/// `SetCenterChunk` + one batch for newly visible chunks.
async fn stream_chunks_around(client: &mut Client, cx: i32, cz: i32) -> Result<(), Error> {
    let wanted = wanted_chunks(&client.server.lobby, cx, cz, client.view_distance);
    let wanted_set: std::collections::HashSet<(i32, i32)> =
        wanted.iter().map(|c| (c.x, c.z)).collect();
    // Padding ring: neighbors of wanted chunks that aren't wanted (or
    // loaded) themselves. Sorted nearest-first so the ground under the
    // player resolves before the horizon.
    let mut ring: Vec<(i32, i32)> = {
        let mut ring_set = std::collections::HashSet::new();
        for &(x, z) in &wanted_set {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let n = (x + dx, z + dz);
                    if !wanted_set.contains(&n) {
                        ring_set.insert(n);
                    }
                }
            }
        }
        let mut ring: Vec<(i32, i32)> = ring_set.into_iter().collect();
        ring.sort_by_key(|&(x, z)| (x - cx) * (x - cx) + (z - cz) * (z - cz));
        ring
    };
    let mut keep: std::collections::HashSet<(i32, i32)> = wanted_set.clone();
    keep.extend(ring.iter().copied());

    let mut gone: Vec<(i32, i32)> = client.sent_chunks.difference(&keep).copied().collect();
    gone.sort_unstable();
    for (x, z) in &gone {
        client.sent_chunks.remove(&(*x, *z));
        client.send(&UnloadChunk { chunk_x: *x, chunk_z: *z }).await?;
    }

    let fresh: Vec<_> =
        wanted.into_iter().filter(|c| !client.sent_chunks.contains(&(c.x, c.z))).collect();
    ring.retain(|pos| !client.sent_chunks.contains(pos));
    if fresh.is_empty() && ring.is_empty() && gone.is_empty() {
        return Ok(());
    }
    eprintln!("stream: center {cx},{cz}: +{} +ring{} -{}", fresh.len(), ring.len(), gone.len());
    if fresh.is_empty() && ring.is_empty() {
        return Ok(());
    }
    client.send(&SetCenterChunk { chunk_x: cx, chunk_z: cz }).await?;
    client.send(&mc_protocol::packets::client::ChunkBatchStart).await?;
    for chunk in &fresh {
        send_chunk(client, chunk).await?;
        client.sent_chunks.insert((chunk.x, chunk.z));
    }
    for &(x, z) in &ring {
        send_empty_chunk(client, x, z).await?;
        client.sent_chunks.insert((x, z));
    }
    client.send(&ChunkBatchFinished { batch_size: (fresh.len() + ring.len()) as i32 }).await?;
    Ok(())
}

/// Apply a validated move; stream chunks when the chunk column changes.
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
        stream_chunks_around(client, cx, cz).await?;
    }
    Ok(())
}
/// Sends the two packets the client renders the player's skin from:
/// `Player Info Update` (Mojang `textures` property verified at login)
/// and `Set Entity Metadata` (skin-parts bitmask from `Client Information`).
async fn send_skin(client: &mut Client) -> Result<(), Error> {
    let (Some(uuid), Some(username)) = (client.ctx.player_uuid, client.ctx.username.as_deref())
    else {
        return Ok(());
    };

    let skin_parts = client.ctx.skin_parts;
    let properties: Vec<ProfileProperty> = client
        .ctx
        .profile_properties
        .iter()
        .map(|property| ProfileProperty {
            name: property.name.as_str(),
            value: property.value.as_str(),
            signature: property.signature.as_deref(),
        })
        .collect();
    // Borrow profile data while encoding, then release it before sending.
    client.packet_buffer.clear();
    PlayerInfoUpdate { uuid, username, properties }.write_packet(&mut client.packet_buffer).await?;
    client.send_encoded_buffer().await?;
    // Entity ID must match the one sent in `PlayLogin`.
    client.send(&SetEntityMetadata { entity_id: PLAYER_ENTITY_ID, skin_parts }).await?;

    Ok(())
}
pub async fn enter_play(client: &mut Client) -> Result<(), Error> {
    send_skin(client).await?;
    let (sx, sy, sz, yaw, pitch) = client.server.lobby.spawn;
    let scx = (sx.floor() as i32) >> 4;
    let scz = (sz.floor() as i32) >> 4;
    client.send(&SetCenterChunk { chunk_x: scx, chunk_z: scz }).await?;
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
    stream_chunks_around(client, scx, scz).await?;

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

    client.send(&PlayKeepAlive { id: 1 }).await?;
    client.keepalive_id = 1;
    client.last_keepalive = Instant::now();

    Ok(())
}
