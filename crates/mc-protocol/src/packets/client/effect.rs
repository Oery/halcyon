//! Cosmetic protocol 777 feedback: swings, entity animations, sounds and particles.

use futures_lite::AsyncWrite;
use macros::{Payload, packet};

use crate::State;
use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::packets::{PacketWriter, Payload};
use crate::types::VarInt;

/// `InteractionHand` ordinals; distinct from the configured dominant arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InteractionHand {
    #[default]
    Main = 0,
    Off = 1,
}

/// `SwingAnimationType` ordinals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwingKind {
    None = 0,
    #[default]
    Whack = 1,
    Stab = 2,
}

/// The default `SwingAnimation` component duration, in client ticks.
pub const SWING_TICKS: i32 = 6;

#[packet(Play, 0x7B, Client)]
pub struct SwingAnimation {
    #[format = "varint"]
    pub entity_id: i32,
    #[format = "varint"]
    pub hand: i32,
    #[format = "varint"]
    pub kind: i32,
    #[format = "varint"]
    pub duration: i32,
}

/// Entity Animation IDs remaining in protocol 777.
pub mod animation {
    pub const CRITICAL_HIT: u8 = 1;
    pub const MAGIC_CRITICAL_HIT: u8 = 2;
}

#[packet(Play, 0x02, Client)]
pub struct EntityAnimation {
    #[format = "varint"]
    pub entity_id: i32,
    pub animation: u8,
}

/// An inline `Holder<SoundEvent>`: zero instead of `registry ID + 1`, then the
/// direct definition. This avoids shipping the sound-event registry table.
#[derive(Clone, Copy, Debug)]
pub struct InlineSound<'p> {
    pub name: &'p str,
    pub fixed_range: Option<f32>,
}

impl<'p> PacketWriter<'p> for InlineSound<'p> {
    fn read(buf: &mut &'p [u8]) -> DecodeResult<Self> {
        if VarInt::read(buf)?.0 != 0 {
            return Err(crate::error::DecodeError::EnumOutOfRange);
        }
        Ok(Self { name: <&str>::read(buf)?, fixed_range: Option::read(buf)? })
    }

    async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        VarInt(0).write(w).await?;
        self.name.write(w).await?;
        self.fixed_range.write(w).await
    }

    fn body_len(&self) -> usize {
        VarInt(0).body_len() + self.name.body_len() + self.fixed_range.body_len()
    }
}

/// `SoundSource` ordinals.
pub mod sound_category {
    pub const MASTER: i32 = 0;
    pub const BLOCK: i32 = 4;
    pub const HOSTILE: i32 = 5;
    pub const NEUTRAL: i32 = 6;
    pub const PLAYER: i32 = 7;
    pub const AMBIENT: i32 = 8;
}

/// Positions are fixed-point eighths of a block.
#[packet(Play, 0x77, Client)]
pub struct SoundEffect<'p> {
    pub sound: InlineSound<'p>,
    #[format = "varint"]
    pub category: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

#[packet(Play, 0x76, Client)]
pub struct EntitySoundEffect<'p> {
    pub sound: InlineSound<'p>,
    #[format = "varint"]
    pub category: i32,
    #[format = "varint"]
    pub entity_id: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

/// `minecraft:particle_type` IDs of data-less particles, from 26.3 registration order.
pub mod particle {
    /// Verified against the official 26.3 PARTICLE_TYPE registry.
    pub const NOTE: i32 = 68;
    pub const CRIT: i32 = 13;
    pub const DAMAGE_INDICATOR: i32 = 14;
    pub const ENCHANTED_HIT: i32 = 25;
    pub const SWEEP_ATTACK: i32 = 77;
}

/// Level Particles for particle types without options; their data is empty.
#[packet(Play, 0x30, Client)]
pub struct LevelParticles {
    #[format = "varint"]
    pub particle: i32,
    pub override_limiter: bool,
    pub always_visible: bool,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub offset_x: f32,
    pub offset_y: f32,
    pub offset_z: f32,
    pub speed_x: f32,
    pub speed_y: f32,
    pub speed_z: f32,
    #[format = "varint"]
    pub count: i32,
    #[format = "varint"]
    pub randomization: i32,
}

#[cfg(test)]
#[path = "../../tests/tests_packets_client_effect.rs"]
mod tests;
