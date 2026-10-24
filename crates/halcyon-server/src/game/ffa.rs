//! Free-for-all: combat opens when the round starts and the last player
//! alive wins. `MatchRunner` handles gathering, the countdown and the end.
use std::collections::{BTreeSet, HashMap};

use super::GameEvent;
use super::matches::{Match, MatchRules, Outcome, ring_spawn};
use crate::instance::GameCtx;
use crate::lifecycle::PlayerRules;
use crate::scoreboard::{
    DisplayOverride, DisplaySlot, LineId, ObjectiveId, SidebarLayout, SidebarLine, Style,
};

pub const MAP: &str = "Parkour Spiral";
const CAPACITY: usize = 8;
/// Players start on a ring around the map spawn.
const SPAWN_RADIUS: f64 = 4.0;

const REMAINING: LineId = LineId::new(0);
const STATUS: LineId = LineId::new(2);
static SIDEBAR: SidebarLayout = SidebarLayout {
    title: "Free for all",
    title_style: Style { color: Some("gold"), bold: true, ..Style::PLAIN },
    ..SidebarLayout::new(
        "Free for all",
        &[
            SidebarLine::new("Waiting for the round"),
            SidebarLine::new(""),
            SidebarLine::new("Status: Waiting"),
        ],
    )
};

#[derive(Default)]
pub struct Ffa {
    alive: BTreeSet<i32>,
    sidebars: HashMap<i32, ObjectiveId>,
}

impl Ffa {
    pub const KIND: &str = "ffa";

    fn update_remaining(&self, ctx: &mut GameCtx<'_>) {
        let _ = ctx.scoreboards().update_lines(
            self.sidebars.values().copied(),
            REMAINING,
            format_args!("Remaining: {}", self.alive.len()),
        );
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
        if let Ok(objective) = boards.create_personal_sidebar(player, &SIDEBAR) {
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
        let _ = ctx.scoreboards().update_lines(
            self.sidebars.values().copied(),
            STATUS,
            format_args!("Status: Alive"),
        );
        ctx.broadcast(super::fight());
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
                let killer = killer.and_then(|killer| ctx.name(killer)).map(str::to_owned);
                ctx.broadcast(super::eliminated(&name, killer.as_deref()));
                // Eliminated players watch from a protected respawn.
                let at = ctx.map_spawn();
                ctx.set_rules(player, PlayerRules::lobby(at));
                if let Some(&objective) = self.sidebars.get(&player) {
                    let _ = ctx.scoreboards().update_line(
                        objective,
                        STATUS,
                        format_args!("Status: Eliminated"),
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
