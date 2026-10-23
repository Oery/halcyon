//! The protected lobby players join first and return to after a game.
//! Matchmaking fills the fullest hub with room before opening another, so a
//! small population shares one lobby and a large one spreads over cores.
use super::{Game, Phase};
use crate::instance::GameCtx;
use crate::lifecycle::PlayerRules;
use crate::simulation::SIMULATION_HZ;

pub const MAP: &str = "Parkour Spiral";
/// Players per hub, bots included, unless HALCYON_HUB_CAPACITY overrides it.
const CAPACITY: usize = 100;
/// An overflow hub stops after this long without players.
const IDLE_SECONDS: u64 = 60;

pub struct Hub {
    /// HALCYON_PVP=1 opens the hub to combat.
    pvp: bool,
    capacity: usize,
    /// Each shard keeps one hub running, so a connection always has a home.
    permanent: bool,
    idle_steps: u64,
    closing: bool,
}

impl Hub {
    pub const KIND: &str = "hub";

    pub fn from_env(permanent: bool) -> Self {
        Self {
            pvp: std::env::var("HALCYON_PVP").is_ok_and(|value| value == "1"),
            capacity: std::env::var("HALCYON_HUB_CAPACITY")
                .ok()
                .and_then(|capacity| capacity.parse().ok())
                .unwrap_or(CAPACITY)
                .max(1),
            permanent,
            idle_steps: 0,
            closing: false,
        }
    }
}

impl Game for Hub {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn phase(&self) -> Phase {
        if self.closing { Phase::Ending } else { Phase::Open }
    }

    /// Players keep their requested spawn, which bots spread around checkpoints.
    fn on_join(&mut self, ctx: &mut GameCtx<'_>, player: i32) {
        let Some(at) = ctx.position(player) else { return };
        let rules = if self.pvp {
            PlayerRules::open(at)
        } else {
            PlayerRules::lobby(at)
        };
        ctx.set_rules(player, rules);
    }

    fn tick(&mut self, ctx: &mut GameCtx<'_>) {
        if self.permanent || self.closing {
            return;
        }
        self.idle_steps = if ctx.player_count() == 0 {
            self.idle_steps + 1
        } else {
            0
        };
        if self.idle_steps >= IDLE_SECONDS * SIMULATION_HZ {
            self.closing = true;
            ctx.close();
        }
    }
}
