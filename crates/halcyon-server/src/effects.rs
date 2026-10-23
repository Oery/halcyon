//! Best-effort cosmetic feedback shared by every observer of a lobby.
//!
//! The owner encodes each effect once into a bounded ring; connections read it
//! with their own cursor. Effects never reserve reliable observer capacity: a
//! connection that falls a full ring behind skips what it missed.

use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use futures_lite::future::block_on;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::{
    EntityAnimation, InlineSound, InteractionHand, LevelParticles, SWING_TICKS, SoundEffect,
    SwingAnimation, SwingKind, particle, sound_category,
};

use crate::lifecycle::Damage;

type Column = (i32, i32);

const CAPACITY: usize = 512;
/// Effects written per drain slice: half the connection's frame budget, so
/// player and world replication keep the rest.
const PER_DRAIN: usize = crate::visibility::WRITE_SLICE_FRAMES / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    /// Observers that currently render this entity.
    Entity(i32),
    /// Observers with this terrain column resident.
    Column(Column),
}

struct Effect {
    audience: Audience,
    /// The actor whose client already predicted this feedback locally.
    except: Option<i32>,
    bytes: Rc<[u8]>,
}

#[derive(Clone, Copy, Debug)]
pub struct Sound {
    pub name: &'static str,
    pub category: i32,
}

// Melee and item actions will play these presets as they arrive.
#[allow(dead_code)]
impl Sound {
    pub const ATTACK_STRONG: Self = Self::player("minecraft:entity.player.attack.strong");
    pub const ATTACK_WEAK: Self = Self::player("minecraft:entity.player.attack.weak");
    pub const ATTACK_CRIT: Self = Self::player("minecraft:entity.player.attack.crit");
    pub const ATTACK_KNOCKBACK: Self = Self::player("minecraft:entity.player.attack.knockback");
    pub const ATTACK_SWEEP: Self = Self::player("minecraft:entity.player.attack.sweep");
    pub const ATTACK_NODAMAGE: Self = Self::player("minecraft:entity.player.attack.nodamage");

    const fn player(name: &'static str) -> Self {
        Self { name, category: sound_category::PLAYER }
    }
}

/// Data-less particle types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Particle(i32);

#[allow(dead_code)]
impl Particle {
    pub const NOTE: Self = Self(particle::NOTE);
    pub const CRIT: Self = Self(particle::CRIT);
    pub const ENCHANTED_HIT: Self = Self(particle::ENCHANTED_HIT);
    pub const DAMAGE_INDICATOR: Self = Self(particle::DAMAGE_INDICATOR);
    pub const SWEEP_ATTACK: Self = Self(particle::SWEEP_ATTACK);
}

#[derive(Clone, Copy, Debug)]
pub struct Particles {
    pub particle: Particle,
    pub pos: [f64; 3],
    pub offset: [f32; 3],
    pub speed: f32,
    pub count: i32,
}

#[derive(Default)]
pub struct EffectLog {
    entries: VecDeque<Effect>,
    /// Sequence number of the next pushed effect.
    head: u64,
}

impl EffectLog {
    pub fn block_break(
        &mut self,
        pos: crate::arena::BlockPos,
        state: halcyon_world::BlockStateId,
        except: Option<i32>,
    ) {
        self.push(
            Audience::Column(crate::arena::column(pos)),
            except,
            &mc_protocol::packets::client::WorldEvent {
                event: 2001,
                location: crate::arena::pack(pos),
                data: i32::from(state),
                global: false,
            },
        );
    }
    fn first(&self) -> u64 {
        self.head - self.entries.len() as u64
    }

    fn push<'p>(&mut self, audience: Audience, except: Option<i32>, packet: &impl Payload<'p>) {
        let mut bytes = Vec::new();
        // Vec's AsyncWrite is immediately ready; this cannot suspend an executor.
        if block_on(packet.write_packet(&mut bytes)).is_err() {
            return;
        }
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Effect { audience, except, bytes: Rc::from(bytes) });
        self.head += 1;
    }

    pub fn swing(&mut self, entity_id: i32, hand: InteractionHand, except: Option<i32>) {
        let packet = SwingAnimation {
            entity_id,
            hand: hand as i32,
            kind: SwingKind::Whack as i32,
            duration: SWING_TICKS,
        };
        self.push(Audience::Entity(entity_id), except, &packet);
    }

    /// Observers play the victim's hurt flash and hurt sound from this event.
    pub fn hurt(&mut self, entity_id: i32, damage: Damage) {
        self.push(Audience::Entity(entity_id), Some(entity_id), &damage.event(entity_id));
    }

    /// Critical-hit particles around an entity (`animation::CRITICAL_HIT`).
    #[allow(dead_code)]
    pub fn animation(&mut self, entity_id: i32, animation: u8) {
        self.push(Audience::Entity(entity_id), None, &EntityAnimation { entity_id, animation });
    }

    #[allow(dead_code)]
    pub fn sound(
        &mut self,
        sound: Sound,
        pos: [f64; 3],
        volume: f32,
        pitch: f32,
        seed: i64,
        except: Option<i32>,
    ) {
        let [x, y, z] = pos.map(|value| (value * 8.0) as i32);
        let packet = SoundEffect {
            sound: InlineSound { name: sound.name, fixed_range: None },
            category: sound.category,
            x,
            y,
            z,
            volume,
            pitch,
            seed,
        };
        self.push(Audience::Column(column(pos)), except, &packet);
    }

    #[allow(dead_code)]
    pub fn particles(&mut self, particles: Particles, except: Option<i32>) {
        let Particles { particle, pos: [x, y, z], offset, speed, count } = particles;
        let packet = LevelParticles {
            particle: particle.0,
            override_limiter: false,
            always_visible: false,
            x,
            y,
            z,
            offset_x: offset[0],
            offset_y: offset[1],
            offset_z: offset[2],
            speed_x: speed,
            speed_y: 0.0,
            speed_z: 0.0,
            count,
            randomization: 0,
        };
        self.push(Audience::Column(column(particles.pos)), except, &packet);
    }
}

fn column([x, _, z]: [f64; 3]) -> Column {
    ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4)
}

/// One connection's read position in its lobby's effect log.
#[derive(Default)]
pub struct EffectCursor {
    next: Option<u64>,
}

impl EffectCursor {
    /// Whether effects remain after the last collection. A new cursor has none,
    /// since it starts at the head rather than replaying history.
    pub fn pending(&self, log: &EffectLog) -> bool {
        self.next.is_some_and(|next| next < log.head)
    }

    /// Append at most `PER_DRAIN` deliverable frames to `out`.
    pub fn collect(
        &mut self,
        log: &EffectLog,
        self_id: i32,
        rendered: impl Fn(i32) -> bool,
        resident: &HashSet<Column>,
        out: &mut Vec<Rc<[u8]>>,
    ) {
        let mut next = self.next.unwrap_or(log.head).max(log.first());
        let skip = (next - log.first()) as usize;
        for effect in log.entries.iter().skip(skip) {
            if out.len() == PER_DRAIN {
                break;
            }
            next += 1;
            if effect.except == Some(self_id) {
                continue;
            }
            let audible = match effect.audience {
                Audience::Entity(id) => id != self_id && rendered(id),
                Audience::Column(column) => resident.contains(&column),
            };
            if audible {
                out.push(effect.bytes.clone());
            }
        }
        self.next = Some(next);
    }
}

#[cfg(test)]
#[path = "tests/tests_effects.rs"]
mod tests;
