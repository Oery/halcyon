//! Free-for-all: combat opens when the round starts and the last player
//! alive wins. `MatchRunner` handles gathering, the countdown and the end.
use std::collections::{BTreeSet, HashMap};

use super::GameEvent;
use super::matches::{Match, MatchRules, Outcome, ring_spawn};
use crate::instance::GameCtx;
use crate::lifecycle::PlayerRules;
use crate::scoreboard::{
    DisplayOverride, DisplaySlot, ObjectiveId, ObjectiveSpec, Span, Style, Text,
};

pub const MAP: &str = "Parkour Spiral";
const CAPACITY: usize = 8;
/// Players start on a ring around the map spawn.
const SPAWN_RADIUS: f64 = 4.0;

#[derive(Default)]
pub struct Ffa {
    alive: BTreeSet<i32>,
    sidebars: HashMap<i32, ObjectiveId>,
}

impl Ffa {
    pub const KIND: &str = "ffa";

    fn update_remaining(&self, ctx: &mut GameCtx<'_>) {
        let remaining = Text::from(format!("Remaining: {}", self.alive.len()));
        for &objective in self.sidebars.values() {
            let _ = ctx.scoreboards().set_sidebar_line(objective, 0, remaining.clone());
        }
    }
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
        self.sidebars.retain(|player, _| ctx.name(*player).is_some());
        let mut boards = ctx.scoreboards();
        if let Ok(objective) = boards.create_personal_objective(player, ObjectiveSpec::default()) {
            let title = Text(vec![Span {
                text: "Free for all".into(),
                style: Style { color: Some("gold".into()), bold: true, ..Default::default() },
            }]);
            let _ = boards.sidebar(
                objective,
                title,
                &[
                    Text::from("Waiting for the round"),
                    Text::from(""),
                    Text::from("Status: Waiting"),
                ],
            );
            let _ = boards.display_for(
                player,
                DisplaySlot::Sidebar,
                DisplayOverride::Objective(objective),
            );
            self.sidebars.insert(player, objective);
        }
    }

    fn on_start(&mut self, ctx: &mut GameCtx<'_>, roster: &[i32]) {
        for (slot, &player) in roster.iter().enumerate() {
            let spawn = ring_spawn(ctx.map_spawn(), SPAWN_RADIUS, slot, CAPACITY);
            ctx.set_rules(player, PlayerRules::open(spawn));
            // A fresh life at the player's ring spawn, with full health.
            ctx.reset(player);
        }
        self.alive = roster.iter().copied().collect();
        self.sidebars.retain(|player, _| ctx.name(*player).is_some());
        self.update_remaining(ctx);
        for &objective in self.sidebars.values() {
            let _ = ctx.scoreboards().set_sidebar_line(objective, 2, "Status: Alive".into());
        }
        ctx.broadcast("Fight!");
    }

    fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {
        match event {
            GameEvent::Left(player) => {
                self.sidebars.remove(&player);
                if self.alive.remove(&player) {
                    self.update_remaining(ctx);
                }
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
                if let Some(&objective) = self.sidebars.get(&player) {
                    let _ = ctx.scoreboards().set_sidebar_line(
                        objective,
                        2,
                        "Status: Eliminated".into(),
                    );
                }
                self.update_remaining(ctx);
            }
            _ => {}
        }
    }

    fn tick(&mut self, _ctx: &mut GameCtx<'_>) -> Option<Outcome> {
        (self.alive.len() <= 1).then(|| Outcome { winners: self.alive.iter().copied().collect() })
    }
}
