use futures_lite::AsyncWriteExt;
use std::time::Instant;

use mc_protocol::packets::Packets;
use mc_protocol::packets::client::{
    ChunkBatchFinished, ChunkData, GameEvent, PlayKeepAlive, PlayerAbilities, SetCenterChunk,
    SetDefaultSpawnPosition, SetHealth, SyncPlayerPosition,
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
            client.pos.x = p.x.clamp(-30_000_000.0, 30_000_000.0);
            client.pos.y = p.feet_y.clamp(-20_000_000.0, 20_000_000.0);
            client.pos.z = p.z.clamp(-30_000_000.0, 30_000_000.0);
            client.pos.on_ground = p.flags & 0x01 != 0;
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
            client.pos.x = p.x.clamp(-30_000_000.0, 30_000_000.0);
            client.pos.y = p.feet_y.clamp(-20_000_000.0, 20_000_000.0);
            client.pos.z = p.z.clamp(-30_000_000.0, 30_000_000.0);
            client.pos.yaw = p.yaw;
            client.pos.pitch = p.pitch;
            client.pos.on_ground = p.flags & 0x01 != 0;
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

/// Burst sent once on Play entry: player state, center chunk, one void
/// chunk, teleport, first keepalive.
fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x & 0x3FFFFFF) as i64) << 38) | (((z & 0x3FFFFFF) as i64) << 12) | ((y & 0xFFF) as i64)
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

    // `Arc` clone ends the lobby borrow; per-send borrows hit the `Arc`s.
    let chunks: Vec<_> =
        client.server.lobby.chunks_by_distance(scx, scz).into_iter().cloned().collect();
    let batch_size = chunks.len() as i32;
    for chunk in &chunks {
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
            .await?;
    }

    client.send(&ChunkBatchFinished { batch_size }).await?;

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
