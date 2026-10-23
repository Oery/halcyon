//! Free-for-all: once enough players gather, a countdown opens combat and
//! the last player alive wins. Everyone then returns to the hub.
use std::collections::BTreeSet;

use super::{Destination, Game, GameEvent, Phase};
use crate::instance::{GameCtx, Transform};
use crate::lifecycle::PlayerRules;
use crate::simulation::SIMULATION_HZ;

pub const MAP: &str = "Parkour Spiral";
const MIN_PLAYERS: usize = 2;
const CAPACITY: usize = 8;
const COUNTDOWN_SECONDS: u64 = 5;
const ENDING_SECONDS: u64 = 5;
/// Players start on a ring around the map spawn.
const SPAWN_RADIUS: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Waiting,
    /// Steps left before combat opens.
    Countdown(u64),
    Fighting,
    /// Steps left before players return to the hub.
    Ending(u64),
    /// Players were sent away; the owner stops once the last one leaves.
    Closing,
}

pub struct Ffa {
    state: State,
    alive: BTreeSet<i32>,
    /// Someone joined, so an empty instance is abandoned rather than new.
    joined: bool,
}

impl Default for Ffa {
    fn default() -> Self {
        Self { state: State::Waiting, alive: BTreeSet::new(), joined: false }
    }
}

impl Ffa {
    pub const KIND: &str = "ffa";

    fn spawn(ctx: &GameCtx<'_>, slot: usize) -> Transform {
        let center = ctx.map_spawn();
        let angle = slot as f64 / CAPACITY as f64 * std::f64::consts::TAU;
        Transform {
            x: center.x + SPAWN_RADIUS * angle.cos(),
            z: center.z + SPAWN_RADIUS * angle.sin(),
            ..center
        }
    }

    fn end(&mut self, ctx: &mut GameCtx<'_>, winner: Option<i32>) {
        match winner.and_then(|winner| ctx.name(winner)).map(str::to_owned) {
            Some(name) => ctx.broadcast(&format!("{name} wins!")),
            None => ctx.broadcast("Nobody wins."),
        }
        self.state = State::Ending(ENDING_SECONDS * SIMULATION_HZ);
    }
}

impl Game for Ffa {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    fn capacity(&self) -> usize {
        CAPACITY
    }

    fn phase(&self) -> Phase {
        match self.state {
            State::Waiting | State::Countdown(_) => Phase::Open,
            State::Fighting => Phase::Running,
            State::Ending(_) | State::Closing => Phase::Ending,
        }
    }

    fn on_join(&mut self, ctx: &mut GameCtx<'_>, player: i32) {
        self.joined = true;
        let waiting = ctx.player_count();
        let spawn = Self::spawn(ctx, waiting.saturating_sub(1));
        ctx.teleport(player, spawn);
        let at = ctx.position(player).unwrap_or(spawn);
        ctx.set_rules(player, PlayerRules::lobby(at));
        ctx.broadcast(&format!(
            "{} joined ({waiting}/{CAPACITY}).",
            ctx.name(player).unwrap_or("?")
        ));
    }

    fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {
        match event {
            GameEvent::Left(player) => {
                self.alive.remove(&player);
            }
            GameEvent::Died { player, killer } if self.alive.remove(&player) => {
                let name = ctx.name(player).unwrap_or("?").to_owned();
                match killer.and_then(|killer| ctx.name(killer)).map(str::to_owned) {
                    Some(killer) => ctx.broadcast(&format!("{name} was eliminated by {killer}.")),
                    None => ctx.broadcast(&format!("{name} was eliminated.")),
                }
                // Eliminated players watch from a protected respawn.
                let at = ctx.map_spawn();
                ctx.set_rules(player, PlayerRules::lobby(at));
            }
            _ => {}
        }
    }

    fn tick(&mut self, ctx: &mut GameCtx<'_>) {
        let count = ctx.player_count();
        self.state = match self.state {
            State::Waiting | State::Countdown(_) if count == 0 && self.joined => {
                ctx.close();
                State::Closing
            }
            State::Waiting if count >= MIN_PLAYERS => {
                ctx.broadcast(&format!("Starting in {COUNTDOWN_SECONDS} seconds."));
                State::Countdown(COUNTDOWN_SECONDS * SIMULATION_HZ)
            }
            State::Countdown(_) if count < MIN_PLAYERS => {
                ctx.broadcast("Not enough players; waiting.");
                State::Waiting
            }
            State::Countdown(0) => {
                // Collected once per round: the loop changes each player.
                let players: Vec<i32> = ctx.players().collect();
                for (slot, &player) in players.iter().enumerate() {
                    let spawn = Self::spawn(ctx, slot);
                    ctx.set_rules(player, PlayerRules::open(spawn));
                    // A fresh life at the player's ring spawn, with full health.
                    ctx.reset(player);
                }
                self.alive = players.iter().copied().collect();
                ctx.broadcast("Fight!");
                State::Fighting
            }
            State::Countdown(steps) => {
                if steps % SIMULATION_HZ == 0 && steps <= 3 * SIMULATION_HZ {
                    ctx.broadcast(&format!("{}...", steps / SIMULATION_HZ));
                }
                State::Countdown(steps - 1)
            }
            State::Fighting if self.alive.len() <= 1 => {
                let winner = self.alive.first().copied();
                self.end(ctx, winner);
                self.state
            }
            State::Ending(0) => {
                let players: Vec<i32> = ctx.players().collect();
                for player in players {
                    ctx.send_to(player, Destination::Hub);
                }
                ctx.close();
                State::Closing
            }
            State::Ending(steps) => State::Ending(steps - 1),
            state => state,
        };
    }
}
