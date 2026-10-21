use crate::error::Error;
use std::time::{Duration, Instant};

use crate::lobby::Transform;

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);

pub const TELEPORT_TIMEOUT: Duration = Duration::from_secs(30);
const DAMAGE_WINDOW: Duration = Duration::from_millis(500);
const DEATH_ANIMATION: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageKind {
    Fall,
    Void,
    Suffocation,
    /// Used by arena damage APIs before a concrete game exists.
    #[allow(dead_code)]
    Game,
}

impl DamageKind {
    /// Indices in the versioned damage_type registry sent during configuration.
    pub fn protocol_id(self) -> i32 {
        match self {
            Self::Fall => 10,
            Self::Void => 32,
            Self::Suffocation => 22,
            Self::Game => 18,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Damage {
    pub kind: DamageKind,
    pub amount: f32,
    pub source: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Teleport {
    pub id: i32,
    pub destination: Transform,
}

/// Deliberate minigame rules; protection is evaluated before damage-type bypasses.
#[derive(Clone, Copy, Debug)]
pub struct PlayerRules {
    pub protected: bool,
    pub fall_damage: bool,
    pub void_damage: bool,
    pub respawn: Transform,
}

impl PlayerRules {
    pub fn lobby(spawn: Transform) -> Self {
        Self { protected: true, fall_damage: false, void_damage: false, respawn: spawn }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DamageResult {
    Ignored,
    Hurt { health: f32, died: bool },
}

/// Gameplay lifecycle, independent of connection/login deadlines and socket I/O.
pub struct PlayerLife {
    health: f32,
    pub rules: PlayerRules,
    pub generation: u64,
    pending: Option<Teleport>,
    next_teleport_id: i32,
    dead_at: Option<Instant>,
    damage_until: Option<Instant>,
    last_damage: f32,
    next_void: Option<Instant>,
}

impl PlayerLife {
    pub fn new(spawn: Transform) -> Self {
        Self {
            health: 20.0,
            rules: PlayerRules::lobby(spawn),
            generation: 0,
            pending: None,
            next_teleport_id: 0,
            dead_at: None,
            damage_until: None,
            last_damage: 0.0,
            next_void: None,
        }
    }

    pub fn health(&self) -> f32 {
        self.health
    }

    pub fn alive(&self) -> bool {
        self.dead_at.is_none()
    }

    pub fn visible(&self, now: Instant) -> bool {
        self.dead_at.is_none_or(|at| now.saturating_duration_since(at) < DEATH_ANIMATION)
    }

    pub fn pending_teleport(&self) -> Option<Teleport> {
        self.pending
    }

    pub fn teleport(&mut self, destination: Transform) -> Option<Teleport> {
        if !destination.is_valid() {
            return None;
        }
        self.next_teleport_id = self.next_teleport_id.checked_add(1)?;
        let teleport = Teleport { id: self.next_teleport_id, destination };
        self.pending = Some(teleport);
        self.next_void = None;
        Some(teleport)
    }

    pub fn confirm(&mut self, id: i32) -> bool {
        if self.pending.is_some_and(|pending| pending.id == id) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    pub fn damage(&mut self, damage: Damage, now: Instant) -> DamageResult {
        if !self.alive()
            || self.rules.protected
            || !damage.amount.is_finite()
            || damage.amount <= 0.0
        {
            return DamageResult::Ignored;
        }
        let amount = if self.damage_until.is_some_and(|until| now < until) {
            if damage.amount <= self.last_damage {
                return DamageResult::Ignored;
            }
            let extra = damage.amount - self.last_damage;
            self.last_damage = damage.amount;
            extra
        } else {
            self.damage_until = Some(now + DAMAGE_WINDOW);
            self.last_damage = damage.amount;
            damage.amount
        };
        self.health = (self.health - amount).max(0.0);
        if self.health == 0.0 {
            self.dead_at = Some(now);
        }
        DamageResult::Hurt { health: self.health, died: !self.alive() }
    }

    pub fn heal(&mut self, amount: f32) -> bool {
        if !self.alive() || !amount.is_finite() || amount <= 0.0 {
            return false;
        }
        let health = (self.health + amount).min(20.0);
        let changed = health != self.health;
        self.health = health;
        changed
    }

    pub fn landing_damage(&mut self, amount: f32, now: Instant) -> DamageResult {
        if !self.rules.fall_damage {
            return DamageResult::Ignored;
        }
        self.damage(Damage { kind: DamageKind::Fall, amount, source: None }, now)
    }

    pub fn void_damage(&mut self, below_world: bool, now: Instant) -> DamageResult {
        if !below_world {
            self.next_void = None;
            return DamageResult::Ignored;
        }
        if !self.rules.void_damage || self.next_void.is_some_and(|next| now < next) {
            return DamageResult::Ignored;
        }
        // Vanilla attempts four points below minY - 64; its hurt window permits
        // another equal hit after half a second, independent of our 120 Hz clock.
        let result = self.damage(Damage { kind: DamageKind::Void, amount: 4.0, source: None }, now);
        if matches!(result, DamageResult::Hurt { .. }) {
            self.next_void = Some(now + DAMAGE_WINDOW);
        }
        result
    }

    pub fn respawn(&mut self) -> bool {
        if self.alive() {
            return false;
        }
        self.reset();
        true
    }

    pub fn reset(&mut self) {
        self.health = 20.0;
        self.dead_at = None;
        self.damage_until = None;
        self.last_damage = 0.0;
        self.next_void = None;
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LoginProgress {
    #[default]
    Start,
    Encryption,
    Acknowledge,
    Complete,
}

impl LoginProgress {
    pub fn advance(&mut self, expected: Self, next: Self) -> Result<(), Error> {
        if *self != expected {
            return Err(Error::UnexpectedResponse);
        }
        *self = next;
        Ok(())
    }
}

pub struct Keepalive {
    next: Instant,
    pending: Option<(i64, Instant)>,
    id: i64,
}

impl Keepalive {
    pub fn new(now: Instant) -> Self {
        Self { next: now, pending: None, id: 0 }
    }

    pub fn deadline(&self) -> Instant {
        self.pending.map_or(self.next, |(_, sent)| sent + KEEPALIVE_TIMEOUT)
    }

    pub fn response_deadline(&self) -> Option<Instant> {
        self.pending.map(|(_, sent)| sent + KEEPALIVE_TIMEOUT)
    }

    pub fn tick(&mut self, now: Instant) -> Result<Option<i64>, Error> {
        if let Some((_, sent)) = self.pending {
            if now >= sent + KEEPALIVE_TIMEOUT {
                return Err(Error::Timeout);
            }
        } else if now >= self.next {
            self.id = self.id.wrapping_add(1);
            self.pending = Some((self.id, now));
            self.next = now + KEEPALIVE_INTERVAL;
            return Ok(Some(self.id));
        }
        Ok(None)
    }

    pub fn acknowledge(&mut self, id: i64) -> Result<(), Error> {
        if !matches!(self.pending, Some((expected, _)) if expected == id) {
            return Err(Error::UnexpectedResponse);
        }
        self.pending = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
