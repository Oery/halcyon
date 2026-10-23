//! Free-for-all: combat opens when the round starts and the last player
//! alive wins. `MatchRunner` handles gathering, the countdown and the end.
use std::collections::BTreeSet;

use super::GameEvent;
use super::matches::{Match, MatchRules, Outcome, ring_spawn};
use crate::instance::GameCtx;
use crate::lifecycle::PlayerRules;

pub const MAP: &str = "Parkour Spiral";
const CAPACITY: usize = 8;
/// Players start on a ring around the map spawn.
const SPAWN_RADIUS: f64 = 4.0;

#[derive(Default)]
pub struct Ffa {
    alive: BTreeSet<i32>,
}

impl Ffa {
    pub const KIND: &str = "ffa";
}

impl Match for Ffa {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    fn rules(&self) -> MatchRules {
        MatchRules { min_players: 2, max_players: CAPACITY, countdown_seconds: 5, end_seconds: 5 }
    }

    fn on_arrive(&mut self, ctx: &mut GameCtx<'_>, player: i32, waiting: usize) {
        let spawn = ring_spawn(ctx.map_spawn(), SPAWN_RADIUS, waiting.saturating_sub(1), CAPACITY);
        ctx.teleport(player, spawn);
        let at = ctx.position(player).unwrap_or(spawn);
        ctx.set_rules(player, PlayerRules::lobby(at));
    }

    fn on_start(&mut self, ctx: &mut GameCtx<'_>, roster: &[i32]) {
        for (slot, &player) in roster.iter().enumerate() {
            let spawn = ring_spawn(ctx.map_spawn(), SPAWN_RADIUS, slot, CAPACITY);
            ctx.set_rules(player, PlayerRules::open(spawn));
            // A fresh life at the player's ring spawn, with full health.
            ctx.reset(player);
        }
        self.alive = roster.iter().copied().collect();
        ctx.broadcast("Fight!");
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

    fn tick(&mut self, _ctx: &mut GameCtx<'_>) -> Option<Outcome> {
        (self.alive.len() <= 1).then(|| Outcome { winners: self.alive.iter().copied().collect() })
    }
}
