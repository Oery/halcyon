//! The linear movement and stationary player-spawn subset of protocol 777.

use futures_lite::AsyncWrite;
use macros::{Payload, packet};
use uuid::Uuid;

use crate::State;
use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::error::{DecodeError, EncodeError};
use crate::packets::{PacketWriter, Payload};
use crate::types::VarInt;

/// The player entry in the Minecraft Java 26.3 entity-type registry.
pub const PLAYER_ENTITY_TYPE: i32 = 159;

/// Protocol 777 Entity/LivingEntity/Avatar metadata, checked against the target
/// client's accessors: flags 0, pose 6, health 9, displayed skin parts 16.
#[derive(Debug)]
pub struct PlayerStateMetadata {
    pub entity_id: i32,
    pub skin_parts: u8,
    pub sneaking: bool,
    pub sprinting: bool,
    pub health: f32,
}

impl<'p> Payload<'p> for PlayerStateMetadata {
    const ID: VarInt = VarInt(0x65);
    const STATE: State = State::Play;

    fn payload_len(&self) -> EncodeResult<usize> {
        Ok(VarInt(self.entity_id).body_len() + 16)
    }

    fn decode_payload(_buf: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }

    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        VarInt(self.entity_id).write(w).await?;
        0u8.write(w).await?;
        VarInt(0).write(w).await?;
        ((u8::from(self.sneaking) << 1) | (u8::from(self.sprinting) << 3)).write(w).await?;
        6u8.write(w).await?;
        VarInt(20).write(w).await?;
        VarInt(if self.health <= 0.0 {
            7
        } else if self.sneaking {
            5
        } else {
            0
        })
        .write(w)
        .await?;
        9u8.write(w).await?;
        VarInt(3).write(w).await?;
        self.health.write(w).await?;
        16u8.write(w).await?;
        VarInt(0).write(w).await?;
        self.skin_parts.write(w).await?;
        0xffu8.write(w).await
    }
}

#[packet(Play, 0x22, Client)]
pub struct EntityStatus {
    pub entity_id: i32,
    pub status: u8,
}

/// Convert degrees to the protocol's unsigned, wrapping 1/256-turn angle.
pub fn encode_angle(degrees: f32) -> u8 {
    ((f64::from(degrees).rem_euclid(360.0) * 256.0 / 360.0).floor() as u16) as u8
}

fn decode_angle(angle: u8) -> f32 {
    f32::from(angle) * 360.0 / 256.0
}

// Keep the public runtime API in degrees while declaring field order once in
// small wire structs. The existing packet macro handles framing and lengths.
macro_rules! wire_payload {
    ($public:ident, $wire:ident, $to_wire:expr, $from_wire:expr) => {
        impl<'p> Payload<'p> for $public {
            const ID: VarInt = <$wire as Payload<'p>>::ID;
            const STATE: State = State::Play;

            fn payload_len(&self) -> EncodeResult<usize> {
                let wire: $wire = ($to_wire)(self);
                wire.payload_len()
            }

            fn decode_payload(buf: &mut &'p [u8]) -> DecodeResult<Self> {
                ($from_wire)(<$wire as Payload<'p>>::decode_payload(buf)?)
            }

            async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
                let wire: $wire = ($to_wire)(self);
                wire.write_payload(w).await
            }
        }
    };
}

/// A remote player spawn. The local player is created by Play Login instead.
#[derive(Debug)]
pub struct SpawnPlayer {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

#[packet(Play, 0x01, Client)]
struct SpawnPlayerWire {
    #[format = "varint"]
    entity_id: i32,
    uuid: Uuid,
    #[format = "varint"]
    entity_type: i32,
    x: f64,
    y: f64,
    z: f64,
    // A zero LpVec3 is exactly one byte, before the three angles in 26.3.
    velocity: u8,
    pitch: u8,
    yaw: u8,
    head_yaw: u8,
    #[format = "varint"]
    data: i32,
}

wire_payload!(
    SpawnPlayer,
    SpawnPlayerWire,
    |p: &SpawnPlayer| SpawnPlayerWire {
        entity_id: p.entity_id,
        uuid: p.uuid,
        entity_type: PLAYER_ENTITY_TYPE,
        x: p.x,
        y: p.y,
        z: p.z,
        velocity: 0,
        pitch: encode_angle(p.pitch),
        yaw: encode_angle(p.yaw),
        head_yaw: encode_angle(p.yaw),
        data: 0,
    },
    |p: SpawnPlayerWire| {
        if p.entity_type != PLAYER_ENTITY_TYPE || p.velocity != 0 || p.data != 0 {
            return Err(DecodeError::EnumOutOfRange);
        }
        Ok(Self {
            entity_id: p.entity_id,
            uuid: p.uuid,
            x: p.x,
            y: p.y,
            z: p.z,
            yaw: decode_angle(p.yaw),
            pitch: decode_angle(p.pitch),
        })
    }
);

#[derive(Debug)]
pub struct MoveEntityPosition {
    pub entity_id: i32,
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub on_ground: bool,
}

#[packet(Play, 0x36, Client)]
struct MoveEntityPositionWire {
    #[format = "varint"]
    entity_id: i32,
    #[format = "varint"]
    properties: i32,
    dx: i16,
    dy: i16,
    dz: i16,
}

wire_payload!(
    MoveEntityPosition,
    MoveEntityPositionWire,
    |p: &MoveEntityPosition| {
        MoveEntityPositionWire {
            entity_id: p.entity_id,
            properties: i32::from(p.on_ground),
            dx: p.dx,
            dy: p.dy,
            dz: p.dz,
        }
    },
    |p: MoveEntityPositionWire| {
        if p.properties & !1 != 0 {
            return Err(DecodeError::EnumOutOfRange);
        }
        Ok(Self {
            entity_id: p.entity_id,
            dx: p.dx,
            dy: p.dy,
            dz: p.dz,
            on_ground: p.properties != 0,
        })
    }
);

#[derive(Debug)]
pub struct MoveEntityPositionRotation {
    pub entity_id: i32,
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

#[packet(Play, 0x37, Client)]
struct MoveEntityPositionRotationWire {
    #[format = "varint"]
    entity_id: i32,
    #[format = "varint"]
    properties: i32,
    dx: i16,
    dy: i16,
    dz: i16,
    yaw: u8,
    pitch: u8,
}

wire_payload!(
    MoveEntityPositionRotation,
    MoveEntityPositionRotationWire,
    |p: &MoveEntityPositionRotation| MoveEntityPositionRotationWire {
        entity_id: p.entity_id,
        properties: i32::from(p.on_ground),
        dx: p.dx,
        dy: p.dy,
        dz: p.dz,
        yaw: encode_angle(p.yaw),
        pitch: encode_angle(p.pitch),
    },
    |p: MoveEntityPositionRotationWire| {
        if p.properties & !1 != 0 {
            return Err(DecodeError::EnumOutOfRange);
        }
        Ok(Self {
            entity_id: p.entity_id,
            dx: p.dx,
            dy: p.dy,
            dz: p.dz,
            yaw: decode_angle(p.yaw),
            pitch: decode_angle(p.pitch),
            on_ground: p.properties != 0,
        })
    }
);

#[derive(Debug)]
pub struct MoveEntityRotation {
    pub entity_id: i32,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

#[packet(Play, 0x39, Client)]
struct MoveEntityRotationWire {
    #[format = "varint"]
    entity_id: i32,
    on_ground: bool,
    yaw: u8,
    pitch: u8,
}

wire_payload!(
    MoveEntityRotation,
    MoveEntityRotationWire,
    |p: &MoveEntityRotation| {
        MoveEntityRotationWire {
            entity_id: p.entity_id,
            on_ground: p.on_ground,
            yaw: encode_angle(p.yaw),
            pitch: encode_angle(p.pitch),
        }
    },
    |p: MoveEntityRotationWire| Ok(Self {
        entity_id: p.entity_id,
        on_ground: p.on_ground,
        yaw: decode_angle(p.yaw),
        pitch: decode_angle(p.pitch),
    })
);

#[derive(Debug)]
pub struct EntityPositionSync {
    pub entity_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

#[packet(Play, 0x23, Client)]
struct EntityPositionSyncWire {
    #[format = "varint"]
    entity_id: i32,
    #[format = "varint"]
    path_type: i32,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
}

wire_payload!(
    EntityPositionSync,
    EntityPositionSyncWire,
    |p: &EntityPositionSync| {
        EntityPositionSyncWire {
            entity_id: p.entity_id,
            path_type: 0,
            x: p.x,
            y: p.y,
            z: p.z,
            yaw: p.yaw,
            pitch: p.pitch,
            on_ground: p.on_ground,
        }
    },
    |p: EntityPositionSyncWire| {
        if p.path_type != 0 {
            return Err(DecodeError::EnumOutOfRange);
        }
        Ok(Self {
            entity_id: p.entity_id,
            x: p.x,
            y: p.y,
            z: p.z,
            yaw: p.yaw,
            pitch: p.pitch,
            on_ground: p.on_ground,
        })
    }
);

#[derive(Debug)]
pub struct SetHeadRotation {
    pub entity_id: i32,
    pub yaw: f32,
}

#[packet(Play, 0x55, Client)]
struct SetHeadRotationWire {
    #[format = "varint"]
    entity_id: i32,
    yaw: u8,
}

wire_payload!(
    SetHeadRotation,
    SetHeadRotationWire,
    |p: &SetHeadRotation| SetHeadRotationWire { entity_id: p.entity_id, yaw: encode_angle(p.yaw) },
    |p: SetHeadRotationWire| Ok(Self { entity_id: p.entity_id, yaw: decode_angle(p.yaw) })
);

/// Borrowed outbound removal list; decoding requires an owned representation.
#[derive(Debug)]
pub struct RemoveEntities<'a> {
    pub entity_ids: &'a [i32],
}

impl<'p> Payload<'p> for RemoveEntities<'_> {
    const ID: VarInt = VarInt(0x4E);
    const STATE: State = State::Play;

    fn payload_len(&self) -> EncodeResult<usize> {
        let count = i32::try_from(self.entity_ids.len()).map_err(|_| EncodeError::InvalidLength)?;
        Ok(VarInt(count).body_len()
            + self.entity_ids.iter().map(|id| VarInt(*id).body_len()).sum::<usize>())
    }

    fn decode_payload(_: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }

    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        let count = i32::try_from(self.entity_ids.len()).map_err(|_| EncodeError::InvalidLength)?;
        VarInt(count).write(w).await?;
        for id in self.entity_ids {
            VarInt(*id).write(w).await?;
        }
        Ok(())
    }
}

/// Borrowed outbound removal list; decoding requires an owned representation.
#[derive(Debug)]
pub struct PlayerInfoRemove<'a> {
    pub uuids: &'a [Uuid],
}

impl<'p> Payload<'p> for PlayerInfoRemove<'_> {
    const ID: VarInt = VarInt(0x46);
    const STATE: State = State::Play;

    fn payload_len(&self) -> EncodeResult<usize> {
        let count = i32::try_from(self.uuids.len()).map_err(|_| EncodeError::InvalidLength)?;
        Ok(VarInt(count).body_len() + self.uuids.len() * 16)
    }

    fn decode_payload(_: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }

    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        let count = i32::try_from(self.uuids.len()).map_err(|_| EncodeError::InvalidLength)?;
        VarInt(count).write(w).await?;
        for uuid in self.uuids {
            uuid.write(w).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_metadata_uses_the_target_clients_serializer_ids() {
        futures_lite::future::block_on(async {
            let packet = PlayerStateMetadata {
                entity_id: 1,
                skin_parts: 0x7f,
                sneaking: true,
                sprinting: true,
                health: 20.0,
            };
            let mut bytes = Vec::new();
            packet.write_payload(&mut bytes).await.unwrap();
            assert_eq!(bytes.len(), packet.payload_len().unwrap());
            assert_eq!(bytes, [1, 0, 0, 0x0a, 6, 20, 5, 9, 3, 0x41, 0xa0, 0, 0, 16, 0, 0x7f, 0xff]);
        });
    }

    fn frame<P: Payload<'static>>(packet: &P) -> Vec<u8> {
        futures_lite::future::block_on(async {
            let mut bytes = Vec::new();
            packet.write_packet(&mut bytes).await.unwrap();
            let mut input = bytes.as_slice();
            let length = VarInt::read(&mut input).unwrap().0 as usize;
            assert_eq!(input.len(), length);
            assert_eq!(VarInt::read(&mut input).unwrap().0, P::ID.0);
            assert_eq!(input.len(), packet.payload_len().unwrap());
            bytes
        })
    }

    #[test]
    fn spawn_uses_player_registry_and_zero_lp_velocity_before_angles() {
        let packet = SpawnPlayer {
            entity_id: 128,
            uuid: Uuid::from_u128(1),
            x: 1.0,
            y: -2.0,
            z: 3.0,
            yaw: 90.0,
            pitch: -90.0,
        };
        let actual = frame(&packet);
        let mut expected = vec![50, 0x01, 0x80, 0x01];
        expected.extend_from_slice(&1u128.to_be_bytes());
        expected.extend_from_slice(&[0x9F, 0x01]);
        expected.extend_from_slice(&1.0f64.to_be_bytes());
        expected.extend_from_slice(&(-2.0f64).to_be_bytes());
        expected.extend_from_slice(&3.0f64.to_be_bytes());
        expected.extend_from_slice(&[0x00, 192, 64, 64, 0x00]);
        assert_eq!(actual, expected);
        let mut input = &actual[2..];
        let decoded = SpawnPlayer::decode_payload(&mut input).unwrap();
        assert_eq!(decoded.entity_id, 128);
        assert_eq!(decoded.uuid, packet.uuid);
        assert_eq!(decoded.yaw, 90.0);
        assert_eq!(decoded.pitch, 270.0);
        assert!(input.is_empty());
    }

    #[test]
    fn movement_uses_properties_before_signed_delta_triplet() {
        assert_eq!(
            frame(&MoveEntityPosition {
                entity_id: 1,
                dx: i16::MIN,
                dy: i16::MAX,
                dz: -1,
                on_ground: true,
            }),
            [9, 0x36, 1, 1, 0x80, 0, 0x7F, 0xFF, 0xFF, 0xFF],
        );
        assert_eq!(
            frame(&MoveEntityPositionRotation {
                entity_id: 1,
                dx: 4096,
                dy: -4096,
                dz: 0,
                yaw: -90.0,
                pitch: 90.0,
                on_ground: false,
            }),
            [11, 0x37, 1, 0, 0x10, 0, 0xF0, 0, 0, 0, 192, 64],
        );
        assert_eq!(
            frame(&MoveEntityRotation { entity_id: 1, yaw: 180.0, pitch: 0.0, on_ground: true }),
            [5, 0x39, 1, 1, 128, 0],
        );
        assert_eq!(frame(&SetHeadRotation { entity_id: 1, yaw: 450.0 }), [3, 0x55, 1, 64],);
    }

    #[test]
    fn absolute_position_uses_linear_path_and_float_angles() {
        let packet = EntityPositionSync {
            entity_id: 3,
            x: 1.0,
            y: 2.0,
            z: -3.0,
            yaw: 12.5,
            pitch: -35.0,
            on_ground: true,
        };
        let mut expected = vec![36, 0x23, 3, 0];
        expected.extend_from_slice(&1.0f64.to_be_bytes());
        expected.extend_from_slice(&2.0f64.to_be_bytes());
        expected.extend_from_slice(&(-3.0f64).to_be_bytes());
        expected.extend_from_slice(&12.5f32.to_be_bytes());
        expected.extend_from_slice(&(-35.0f32).to_be_bytes());
        expected.push(1);
        assert_eq!(frame(&packet), expected);
        let mut input = &expected[2..];
        let decoded = EntityPositionSync::decode_payload(&mut input).unwrap();
        assert_eq!(decoded.x, packet.x);
        assert_eq!(decoded.yaw, packet.yaw);
        assert!(decoded.on_ground);
        assert!(input.is_empty());
    }

    #[test]
    fn removal_lists_use_varint_counts_and_entity_ids() {
        assert_eq!(
            frame(&RemoveEntities { entity_ids: &[1, 128, -1] }),
            [10, 0x4E, 3, 1, 128, 1, 255, 255, 255, 255, 15]
        );
        assert_eq!(frame(&RemoveEntities { entity_ids: &[] }), [2, 0x4E, 0]);
        let mut expected = vec![34, 0x46, 2];
        expected.extend_from_slice(&0u128.to_be_bytes());
        expected.extend_from_slice(&1u128.to_be_bytes());
        assert_eq!(
            frame(&PlayerInfoRemove { uuids: &[Uuid::nil(), Uuid::from_u128(1)] }),
            expected
        );
    }

    #[test]
    fn angles_wrap_and_quantize_negative_and_large_degrees() {
        assert_eq!(encode_angle(0.0), 0);
        assert_eq!(encode_angle(360.0), 0);
        assert_eq!(encode_angle(-1.0), 255);
        assert_eq!(encode_angle(90.0), 64);
        assert_eq!(encode_angle(-450.0), 192);
        assert_eq!(encode_angle(f32::MAX), 0);
    }
}
