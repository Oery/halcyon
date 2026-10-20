use futures_lite::AsyncWriteExt;
use std::time::Instant;

use mc_protocol::packets::Packets;
use mc_protocol::packets::client::{
    ChunkBatchFinished, ChunkData, GameEvent, PlayKeepAlive, PlayerAbilities, SetCenterChunk,
    SetDefaultSpawnPosition, SetHealth, SyncPlayerPosition, UnloadChunk,
};
use mc_protocol::types::{Heightmap, LightData};

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

/// Send one pre-encoded chunk column.
async fn send_chunk(
    client: &mut Client,
    chunk: &std::sync::Arc<halcyon_world::Chunk>,
) -> Result<(), Error> {
    client
        .send(&ChunkData {
            chunk_x: chunk.x,
            chunk_z: chunk.z,
            heightmaps: vec![Heightmap {
                kind: 4, // MOTION_BLOCKING
                data: chunk.heights.to_vec(),
            }],
            data: &chunk.data,
            block_entities: Vec::new(),
            light: LightData {
                sky_mask: Vec::new(),
                block_mask: Vec::new(),
                // 26 sections + 2 bits: lowest 4 bytes set, trailing zero
                // bytes trimmed by toByteArray.
                empty_sky_mask: vec![0xFF, 0xFF, 0xFF, 0x0F],
                empty_block_mask: vec![0xFF, 0xFF, 0xFF, 0x0F],
                sky_light: Vec::new(),
                block_light: Vec::new(),
            },
        })
        .await
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
/// chunk: unload far chunks first, then `SetCenterChunk` + one batch for
/// newly visible chunks.
async fn stream_chunks_around(client: &mut Client, cx: i32, cz: i32) -> Result<(), Error> {
    let wanted = wanted_chunks(&client.server.lobby, cx, cz, client.view_distance);
    let wanted_set: std::collections::HashSet<(i32, i32)> =
        wanted.iter().map(|c| (c.x, c.z)).collect();

    let mut gone: Vec<(i32, i32)> = client.sent_chunks.difference(&wanted_set).copied().collect();
    gone.sort_unstable();
    for (x, z) in &gone {
        client.sent_chunks.remove(&(*x, *z));
        client.send(&UnloadChunk { chunk_x: *x, chunk_z: *z }).await?;
    }

    let fresh: Vec<_> =
        wanted.into_iter().filter(|c| !client.sent_chunks.contains(&(c.x, c.z))).collect();
    if fresh.is_empty() && gone.is_empty() {
        return Ok(());
    }
    eprintln!("stream: center {cx},{cz}: +{} -{}", fresh.len(), gone.len());
    if fresh.is_empty() {
        return Ok(());
    }
    client.send(&SetCenterChunk { chunk_x: cx, chunk_z: cz }).await?;
    client.send(&mc_protocol::packets::client::ChunkBatchStart).await?;
    for chunk in &fresh {
        send_chunk(client, chunk).await?;
        client.sent_chunks.insert((chunk.x, chunk.z));
    }
    client.send(&ChunkBatchFinished { batch_size: fresh.len() as i32 }).await?;
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
pub async fn enter_play(client: &mut Client) -> Result<(), Error> {
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
