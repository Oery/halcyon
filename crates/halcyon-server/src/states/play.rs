use std::time::Instant;

use mc_protocol::packets::Packets;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::{
    ChunkBatchFinished, ChunkData, PlayKeepAlive, SetCenterChunk, SyncPlayerPosition,
};
use mc_protocol::types::LightData;

use crate::Client;

pub async fn handle_packet(pkt: Packets<'_>, _client: &mut Client) {
    match pkt {
        Packets::ConfirmTeleportation(_)
        | Packets::PlayKeepAliveResponse(_)
        | Packets::ChunkBatchReceived(_)
        | Packets::SetPlayerPosition(_)
        | Packets::SetPlayerPositionRotation(_)
        | Packets::SetPlayerRotation(_)
        | Packets::ClientTickEnd(_)
        | Packets::PlayerLoaded(_) => {}
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}

/// Burst sent once on Play entry: center chunk, one void chunk, teleport, first keepalive.
pub async fn enter_play(client: &mut Client) {
    SetCenterChunk { chunk_x: 0, chunk_z: 0 }.write_packet(&mut client.stream).await.unwrap();
    mc_protocol::packets::client::ChunkBatchStart.write_packet(&mut client.stream).await.unwrap();

    // 24 sections: overworld dimension_type (min_y -64, height 384).
    // Each: block_count 0, fluid_count 0, blocks BPE 0 + air id 0,
    // biomes BPE 0 + plains index 0. 8 bytes per section = 192 bytes.
    let mut data = Vec::with_capacity(24 * 8);
    for _ in 0..24 {
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    ChunkData {
        chunk_x: 0,
        chunk_z: 0,
        heightmaps: Vec::new(),
        data,
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
    }
    .write_packet(&mut client.stream)
    .await
    .unwrap();

    ChunkBatchFinished { batch_size: 1 }.write_packet(&mut client.stream).await.unwrap();

    SyncPlayerPosition {
        teleport_id: 1,
        x: 8.5,
        y: 64.0,
        z: 8.5,
        vel_x: 0.0,
        vel_y: 0.0,
        vel_z: 0.0,
        yaw: 0.0,
        pitch: 0.0,
        flags: 0,
    }
    .write_packet(&mut client.stream)
    .await
    .unwrap();

    PlayKeepAlive { id: 1 }.write_packet(&mut client.stream).await.unwrap();
    client.keepalive_id = 1;
    client.last_keepalive = Instant::now();
}
