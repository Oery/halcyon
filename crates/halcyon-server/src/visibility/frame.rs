//! Encode player presentation against the last delivered baseline.

use std::rc::Rc;

use futures_lite::future::block_on;
use mc_protocol::error::EncodeError;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::{
    EntityPositionSync, EntityStatus, MoveEntityPosition, MoveEntityPositionRotation,
    MoveEntityRotation, PlayerStateMetadata, SetEquipment, SetHeadRotation, SpawnPlayer,
    encode_angle,
};

use super::{Delivery, Frame, Rendered};
use crate::instance::PlayerSnapshot;

pub(super) fn rendered(player: &PlayerSnapshot) -> Rendered {
    Rendered {
        transform: player.transform,
        skin_parts: player.skin_parts,
        revision: player.revision,
        sneaking: player.sneaking,
        pose: player.pose,
        swimming: player.swimming,
        sprinting: player.sprinting,
        using: player.using,
        health: player.health,
        equipment: player.equipment.clone(),
        life_generation: player.life_generation,
    }
}

pub(super) fn encode<'p>(packet: &impl Payload<'p>) -> Result<Rc<[u8]>, EncodeError> {
    let mut bytes = Vec::new();
    append(packet, &mut bytes)?;
    Ok(Rc::from(bytes))
}

/// The entry body of a single-player `PlayerInfoUpdate` frame: everything
/// after the action mask and entry count.
pub(super) fn roster_entry(info: &[u8]) -> Option<&[u8]> {
    use mc_protocol::PacketWriter;
    use mc_protocol::packets::client::PlayerInfoUpdate;
    use mc_protocol::types::VarInt;
    let mut bytes = info;
    let length = VarInt::read(&mut bytes).ok()?.0;
    if usize::try_from(length).ok()? != bytes.len()
        || VarInt::read(&mut bytes).ok()?.0 != PlayerInfoUpdate::ID.0
        || u8::read(&mut bytes).ok()? != 0x09
        || VarInt::read(&mut bytes).ok()?.0 != 1
    {
        return None;
    }
    Some(bytes)
}

/// One `PlayerInfoUpdate` carrying every entry, with the same action mask.
pub(super) fn roster_frame(entries: &[&[u8]]) -> Result<Rc<[u8]>, EncodeError> {
    use mc_protocol::PacketWriter;
    use mc_protocol::packets::client::PlayerInfoUpdate;
    use mc_protocol::types::VarInt;
    let count = VarInt(entries.len() as i32);
    let body: usize = entries.iter().map(|entry| entry.len()).sum();
    let length = PlayerInfoUpdate::ID.body_len() + 1 + count.body_len() + body;
    let mut bytes = Vec::with_capacity(length + 5);
    block_on(async {
        VarInt(length as i32).write(&mut bytes).await?;
        PlayerInfoUpdate::ID.write(&mut bytes).await?;
        0x09u8.write(&mut bytes).await?;
        count.write(&mut bytes).await
    })?;
    for entry in entries {
        bytes.extend_from_slice(entry);
    }
    Ok(Rc::from(bytes))
}

fn append<'p>(packet: &impl Payload<'p>, bytes: &mut Vec<u8>) -> Result<(), EncodeError> {
    // Vec's AsyncWrite is immediately ready; this cannot suspend an executor.
    block_on(packet.write_packet(bytes))
}

pub(super) fn spawn_frame(player: PlayerSnapshot) -> Result<Frame, EncodeError> {
    let p = player.transform;
    let mut bytes = Vec::with_capacity(96);
    append(
        &SpawnPlayer {
            entity_id: player.entity_id,
            uuid: player.profile.uuid,
            x: p.x,
            y: p.y,
            z: p.z,
            yaw: p.yaw,
            pitch: p.pitch,
        },
        &mut bytes,
    )?;
    append(&metadata(&player), &mut bytes)?;
    append(&SetHeadRotation { entity_id: player.entity_id, yaw: p.yaw }, &mut bytes)?;
    let mut frames = 3;
    if player.equipment.iter().any(|stack| !stack.is_empty()) {
        append(
            &SetEquipment { entity_id: player.entity_id, slots: &player.equipment },
            &mut bytes,
        )?;
        frames += 1;
    }
    Ok(Frame {
        bytes: Rc::from(bytes),
        frames,
        delivery: Delivery::Spawn(player),
        unload_barrier: false,
    })
}

pub(super) fn movement_frame(
    player: PlayerSnapshot,
    previous: &Rendered,
) -> Result<Option<Frame>, EncodeError> {
    let id = player.entity_id;
    let p = player.transform;
    let old = previous.transform;
    let changed_position = p.x != old.x || p.y != old.y || p.z != old.z;
    let changed_rotation = encode_angle(p.yaw) != encode_angle(old.yaw)
        || encode_angle(p.pitch) != encode_angle(old.pitch);
    let mut bytes = Vec::with_capacity(96);
    let mut frames = 0;
    if changed_position {
        let deltas = [
            quantize_position(p.x) - quantize_position(old.x),
            quantize_position(p.y) - quantize_position(old.y),
            quantize_position(p.z) - quantize_position(old.z),
        ];
        if deltas.iter().all(|delta| *delta >= i16::MIN as f64 && *delta <= i16::MAX as f64) {
            let [dx, dy, dz] = [deltas[0] as i16, deltas[1] as i16, deltas[2] as i16];
            if changed_rotation {
                append(
                    &MoveEntityPositionRotation {
                        entity_id: id,
                        dx,
                        dy,
                        dz,
                        yaw: p.yaw,
                        pitch: p.pitch,
                        on_ground: p.on_ground,
                    },
                    &mut bytes,
                )?;
            } else {
                append(
                    &MoveEntityPosition { entity_id: id, dx, dy, dz, on_ground: p.on_ground },
                    &mut bytes,
                )?;
            }
        } else {
            append(
                &EntityPositionSync {
                    entity_id: id,
                    x: p.x,
                    y: p.y,
                    z: p.z,
                    yaw: p.yaw,
                    pitch: p.pitch,
                    on_ground: p.on_ground,
                },
                &mut bytes,
            )?;
        }
        frames += 1;
    } else if changed_rotation || p.on_ground != old.on_ground {
        append(
            &MoveEntityRotation {
                entity_id: id,
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: p.on_ground,
            },
            &mut bytes,
        )?;
        frames += 1;
    }
    if encode_angle(p.yaw) != encode_angle(old.yaw) {
        append(&SetHeadRotation { entity_id: id, yaw: p.yaw }, &mut bytes)?;
        frames += 1;
    }
    if player.skin_parts != previous.skin_parts
        || player.sneaking != previous.sneaking
        || player.pose != previous.pose
        || player.swimming != previous.swimming
        || player.sprinting != previous.sprinting
        || player.using != previous.using
        || player.health != previous.health
    {
        append(&metadata(&player), &mut bytes)?;
        frames += 1;
    }
    if player.health <= 0.0 && previous.health > 0.0 {
        append(&EntityStatus { entity_id: id, status: 3 }, &mut bytes)?;
        frames += 1;
    }
    if !Rc::ptr_eq(&player.equipment, &previous.equipment) && player.equipment != previous.equipment
    {
        append(&SetEquipment { entity_id: id, slots: &player.equipment }, &mut bytes)?;
        frames += 1;
    }
    Ok((frames != 0).then(|| Frame {
        bytes: Rc::from(bytes),
        frames,
        delivery: Delivery::Move(player),
        unload_barrier: false,
    }))
}

fn metadata(player: &PlayerSnapshot) -> PlayerStateMetadata {
    PlayerStateMetadata {
        entity_id: player.entity_id,
        skin_parts: player.skin_parts,
        sneaking: player.sneaking,
        pose: player.pose,
        swimming: player.swimming,
        sprinting: player.sprinting,
        using: player.using,
        health: player.health,
    }
}

fn quantize_position(value: f64) -> f64 {
    // VecDeltaCodec uses Java Math.round, whose negative ties round upward.
    (value * 4096.0 + 0.5).floor()
}
