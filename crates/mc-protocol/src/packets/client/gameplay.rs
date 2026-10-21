//! Inventory publication and reusable player lifecycle packets for protocol 777.
use futures_lite::AsyncWrite;
use macros::{Payload, packet};

use crate::State;
use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::error::DecodeError;
use crate::packets::{PacketWriter, Payload};
use crate::types::{ItemStack, Nbt, VarInt};

#[packet(Play, 0x6B, Client)]
pub struct SetHeldItemResponse {
    #[format = "varint"]
    pub slot: i32,
}

#[derive(Debug)]
pub struct SetContainerContent<'p> {
    pub window_id: i32,
    pub state_id: i32,
    pub slots: &'p [ItemStack],
    pub carried: &'p ItemStack,
}

impl<'p> Payload<'p> for SetContainerContent<'p> {
    const ID: VarInt = VarInt(0x12);
    const STATE: State = State::Play;
    fn payload_len(&self) -> EncodeResult<usize> {
        Ok(VarInt(self.window_id).body_len()
            + VarInt(self.state_id).body_len()
            + VarInt(self.slots.len() as i32).body_len()
            + self.slots.iter().map(ItemStack::body_len).sum::<usize>()
            + self.carried.body_len())
    }
    fn decode_payload(_input: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }
    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        VarInt(self.window_id).write(w).await?;
        VarInt(self.state_id).write(w).await?;
        VarInt(self.slots.len() as i32).write(w).await?;
        for item in self.slots {
            item.write(w).await?;
        }
        self.carried.write(w).await
    }
}

#[derive(Debug)]
pub struct SetContainerSlot<'p> {
    pub window_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub stack: &'p ItemStack,
}

impl<'p> Payload<'p> for SetContainerSlot<'p> {
    const ID: VarInt = VarInt(0x14);
    const STATE: State = State::Play;
    fn payload_len(&self) -> EncodeResult<usize> {
        Ok(VarInt(self.window_id).body_len()
            + VarInt(self.state_id).body_len()
            + 2
            + self.stack.body_len())
    }
    fn decode_payload(_input: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }
    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        VarInt(self.window_id).write(w).await?;
        VarInt(self.state_id).write(w).await?;
        self.slot.write(w).await?;
        self.stack.write(w).await
    }
}

#[derive(Debug)]
pub struct SetCursorItem<'p> {
    pub stack: &'p ItemStack,
}

impl<'p> Payload<'p> for SetCursorItem<'p> {
    const ID: VarInt = VarInt(0x62);
    const STATE: State = State::Play;
    fn payload_len(&self) -> EncodeResult<usize> {
        Ok(self.stack.body_len())
    }
    fn decode_payload(_input: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }
    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        self.stack.write(w).await
    }
}

/// Main hand, offhand, feet, legs, chest, head. One immutable equipment image
/// serves initial visibility and coalesced subsequent updates.
#[derive(Debug)]
pub struct SetEquipment<'p> {
    pub entity_id: i32,
    pub slots: &'p [ItemStack; 6],
}

impl<'p> Payload<'p> for SetEquipment<'p> {
    const ID: VarInt = VarInt(0x68);
    const STATE: State = State::Play;
    fn payload_len(&self) -> EncodeResult<usize> {
        Ok(VarInt(self.entity_id).body_len()
            + 6
            + self.slots.iter().map(ItemStack::body_len).sum::<usize>())
    }
    fn decode_payload(_input: &mut &'p [u8]) -> DecodeResult<Self> {
        Err(DecodeError::EnumOutOfRange)
    }
    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        VarInt(self.entity_id).write(w).await?;
        for (slot, item) in self.slots.iter().enumerate() {
            (slot as u8 | if slot < 5 { 0x80 } else { 0 }).write(w).await?;
            item.write(w).await?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct DeathLocation<'p> {
    pub dimension: &'p str,
    pub location: i64,
}

impl<'p> PacketWriter<'p> for DeathLocation<'p> {
    fn read(input: &mut &'p [u8]) -> DecodeResult<Self> {
        Ok(Self { dimension: <&str>::read(input)?, location: i64::read(input)? })
    }
    async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        self.dimension.write(w).await?;
        self.location.write(w).await
    }
    fn body_len(&self) -> usize {
        self.dimension.body_len() + 8
    }
}

#[packet(Play, 0x54, Client)]
pub struct Respawn<'p> {
    #[format = "varint"]
    pub dimension_type: i32,
    pub dimension_name: &'p str,
    pub hashed_seed: i64,
    #[format = "varint"]
    pub game_mode: i32,
    #[format = "varint"]
    pub previous_game_mode: i32,
    pub is_debug: bool,
    pub is_flat: bool,
    pub death_location: Option<DeathLocation<'p>>,
    #[format = "varint"]
    pub portal_cooldown: i32,
    #[format = "varint"]
    pub sea_level: i32,
    pub data_kept: u8,
}

#[packet(Play, 0x45, Client)]
pub struct CombatKill<'p> {
    #[format = "varint"]
    pub player_id: i32,
    pub message: Nbt<'p>,
}

#[derive(Debug)]
pub struct DamagePosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl PacketWriter<'_> for DamagePosition {
    fn read(input: &mut &[u8]) -> DecodeResult<Self> {
        Ok(Self { x: f64::read(input)?, y: f64::read(input)?, z: f64::read(input)? })
    }
    async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        self.x.write(w).await?;
        self.y.write(w).await?;
        self.z.write(w).await
    }
    fn body_len(&self) -> usize {
        24
    }
}

#[packet(Play, 0x19, Client)]
pub struct DamageEvent {
    #[format = "varint"]
    pub entity_id: i32,
    #[format = "varint"]
    pub source_type_id: i32,
    /// Protocol entity ID plus one; zero represents no source entity.
    #[format = "varint"]
    pub source_cause_id: i32,
    #[format = "varint"]
    pub source_direct_id: i32,
    pub source_position: Option<DamagePosition>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_event_entity_ids_remain_varints_after_documentation_attributes() {
        futures_lite::future::block_on(async {
            let event = DamageEvent {
                entity_id: 1,
                source_type_id: 18,
                source_cause_id: 0,
                source_direct_id: 0,
                source_position: None,
            };
            let mut bytes = Vec::new();
            event.write_packet(&mut bytes).await.unwrap();
            assert_eq!(bytes, [6, 0x19, 1, 18, 0, 0, 0]);
        });
    }

    #[test]
    fn inventory_equipment_and_respawn_lengths_match_frames() {
        futures_lite::future::block_on(async {
            let empty = ItemStack::default();
            let slots = std::array::from_fn(|_| empty.clone());
            let packet = SetEquipment { entity_id: 1000, slots: &slots };
            let mut bytes = Vec::new();
            packet.write_packet(&mut bytes).await.unwrap();
            assert_eq!(bytes.len(), packet.payload_len().unwrap() + 2);
            assert_eq!(&bytes[4..], &[0x80, 0, 0x81, 0, 0x82, 0, 0x83, 0, 0x84, 0, 5, 0]);
            let respawn = Respawn {
                dimension_type: 0,
                dimension_name: "minecraft:overworld",
                hashed_seed: 0,
                game_mode: 0,
                previous_game_mode: 1,
                is_debug: false,
                is_flat: false,
                death_location: Some(DeathLocation {
                    dimension: "minecraft:overworld",
                    location: 1,
                }),
                portal_cooldown: 0,
                sea_level: 63,
                data_kept: 0,
            };
            bytes.clear();
            respawn.write_packet(&mut bytes).await.unwrap();
            let mut input = bytes.as_slice();
            let length = VarInt::read(&mut input).unwrap().0 as usize;
            assert_eq!(length, input.len());
            VarInt::read(&mut input).unwrap();
            Respawn::decode_payload(&mut input).unwrap();
            assert!(input.is_empty());
        });
    }
}
