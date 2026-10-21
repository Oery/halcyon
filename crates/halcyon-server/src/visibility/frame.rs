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
use crate::lobby::PlayerSnapshot;

pub(super) fn rendered(player: &PlayerSnapshot) -> Rendered {
    Rendered {
        transform: player.transform,
        skin_parts: player.skin_parts,
        revision: player.revision,
        sneaking: player.sneaking,
        crouching: player.crouching,
        sprinting: player.sprinting,
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
        || player.crouching != previous.crouching
        || player.sprinting != previous.sprinting
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
        crouching: player.crouching,
        sprinting: player.sprinting,
        health: player.health,
    }
}

fn quantize_position(value: f64) -> f64 {
    // VecDeltaCodec uses Java Math.round, whose negative ties round upward.
    (value * 4096.0 + 0.5).floor()
}
