//! The protected lobby players join first and return to after a game.
use super::Game;
use crate::instance::GameCtx;
use crate::lifecycle::PlayerRules;

pub const MAP: &str = "Parkour Spiral";

pub struct Hub {
    /// HALCYON_PVP=1 opens the hub to combat.
    pvp: bool,
}

impl Hub {
    pub const KIND: &str = "hub";

    pub fn from_env() -> Self {
        Self { pvp: std::env::var("HALCYON_PVP").is_ok_and(|value| value == "1") }
    }
}

impl Game for Hub {
    fn kind(&self) -> &'static str {
        Self::KIND
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
}
