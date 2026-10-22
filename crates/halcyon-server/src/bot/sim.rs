//! Headless bots: the full brain, motor and physics stack without a lobby,
//! for reproducible scenarios and tests.

use halcyon_world::BlockView;

use super::brain::Brain;
use super::nav::NavService;
use super::profile::BotProfile;
use crate::lobby::Transform;
use crate::movement::MovementState;

/// Below this the server's void damage would kill the player.
const VOID_Y: f64 = (halcyon_world::MIN_BLOCK_Y - 64) as f64;

pub struct SimBot {
    pub brain: Brain,
    pub state: MovementState,
    spawn: Transform,
    health: f32,
    pub max_y: f64,
    pub deaths: u32,
    /// Returns to a checkpoint after getting trapped.
    pub resets: u32,
}

impl SimBot {
    pub fn new(profile: BotProfile, seed: u64, spawn: Transform) -> Self {
        let mut state = MovementState::new(spawn);
        state.place(spawn);
        Self {
            brain: Brain::new(profile, seed, spawn),
            state,
            spawn,
            health: 20.0,
            max_y: spawn.y,
            deaths: 0,
            resets: 0,
        }
    }

    pub fn position(&self) -> Transform {
        self.state.transform()
    }

    /// One 20 Hz tick: decide, then simulate exactly as the lobby owner does.
    pub fn step(&mut self, nav: &mut NavService, view: &impl BlockView) {
        let input = self.brain.tick(self.state.transform(), nav, view);
        let advance = self.state.step(view, input);
        if let Some(damage) = advance.landed_fall_damage {
            self.health -= damage;
        }
        if let Some(reset) = self.brain.take_reset(nav) {
            let to = reset.map_or(self.spawn, |cell| Transform {
                x: f64::from(cell[0]) + 0.5,
                y: f64::from(cell[1]),
                z: f64::from(cell[2]) + 0.5,
                ..self.spawn
            });
            self.state.place(to);
            self.resets += 1;
        }
        let position = self.state.transform();
        self.max_y = self.max_y.max(position.y);
        if self.health <= 0.0 || position.y < VOID_Y {
            self.deaths += 1;
            self.health = 20.0;
            self.state.place(self.spawn);
        }
    }
}

#[cfg(test)]
#[path = "../tests/tests_bot_sim.rs"]
mod tests;
