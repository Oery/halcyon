//! The bot's hands: turns intents into player inputs the way a person would,
//! with eased, rate-limited camera turns, tremor, committed aim error and the
//! occasional late key press. Every higher layer acts through this.

use super::profile::BotProfile;
use super::rng::Rng;
use crate::movement::MovementInput;

/// What a higher layer wants the hands to do this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Intent {
    pub yaw: f32,
    pub pitch: f32,
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sprint: bool,
    pub sneak: bool,
    /// Executing a committed maneuver: aim error is sampled once at the start
    /// and the camera tracks faster.
    pub precise: bool,
    /// Smooth pursuit: how fast the aim point is moving across the view, in
    /// degrees per tick, added to the corrective turn like a human tracking
    /// a moving target.
    pub yaw_rate: f32,
    pub pitch_rate: f32,
}

impl Intent {
    /// Ideal hands: exact heading and timing. Used to validate maneuvers.
    pub fn exact(self) -> MovementInput {
        MovementInput {
            forward: self.forward,
            backward: self.backward,
            left: self.left,
            right: self.right,
            jumping: self.jump,
            sneaking: self.sneak,
            sprinting: self.sprint,
            yaw: self.yaw,
            pitch: self.pitch,
        }
    }
}

pub struct Motor {
    yaw: f32,
    pitch: f32,
    precise: bool,
    aim_error: f32,
    jump_held: bool,
    jump_late: bool,
}

impl Motor {
    pub fn new(yaw: f32, pitch: f32) -> Self {
        Self { yaw, pitch, precise: false, aim_error: 0.0, jump_held: false, jump_late: false }
    }

    pub fn tick(&mut self, intent: Intent, profile: BotProfile, rng: &mut Rng) -> MovementInput {
        if intent.precise && !self.precise {
            self.aim_error = rng.normal() as f32 * profile.aim_sigma();
        }
        if !intent.precise {
            self.aim_error = 0.0;
        }
        self.precise = intent.precise;

        let tremor = f64::from(profile.tremor());
        let shake = [rng.range(-tremor, tremor) as f32, rng.range(-tremor, tremor) as f32 * 0.5];
        self.turn(intent, profile, shake);

        // A fresh press may slip one tick; a held key never repeats the slip.
        let mut jumping = intent.jump;
        if intent.jump && !self.jump_held && !self.jump_late && rng.chance(profile.timing_slip()) {
            self.jump_late = true;
            jumping = false;
        } else if self.jump_late {
            self.jump_late = false;
            jumping = true;
        }
        self.jump_held = intent.jump;

        MovementInput { jumping, yaw: self.yaw, pitch: self.pitch, ..intent.exact() }
    }

    /// Noise-free hands that turn at `profile`'s pace: exact aim and timing,
    /// but the camera still takes ticks to come round. Used to validate
    /// maneuvers against the turning bots actually have.
    pub fn steady(&mut self, intent: Intent, profile: BotProfile) -> MovementInput {
        self.aim_error = 0.0;
        self.precise = intent.precise;
        self.turn(intent, profile, [0.0; 2]);
        MovementInput { yaw: self.yaw, pitch: self.pitch, ..intent.exact() }
    }

    /// Eased, rate-limited camera motion toward the intent, plus `shake`.
    fn turn(&mut self, intent: Intent, profile: BotProfile, shake: [f32; 2]) {
        let target = intent.yaw + self.aim_error;
        let delta = wrap_degrees(target - self.yaw);
        let easing = if intent.precise { 0.6 } else { profile.turn_easing() };
        let turn = delta * easing + intent.yaw_rate;
        self.yaw += turn.clamp(-profile.turn_rate(), profile.turn_rate()) + shake[0];
        self.yaw = wrap_degrees(self.yaw);
        // Wrists move the mouse vertically slower than across.
        let rise = (intent.pitch - self.pitch) * easing + intent.pitch_rate;
        let limit = profile.turn_rate() * 0.6;
        self.pitch += rise.clamp(-limit, limit) + shake[1];
        self.pitch = self.pitch.clamp(-90.0, 90.0);
    }
}

/// Wrap an angle to `[-180, 180)`.
pub fn wrap_degrees(angle: f32) -> f32 {
    (angle + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
#[path = "../tests/tests_bot_motor.rs"]
mod tests;
