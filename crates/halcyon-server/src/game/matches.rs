//! Round-based games. A `Match` declares its `MatchRules` and implements only
//! the round; `MatchRunner` adapts it to `Game` and owns everything around
//! it: gathering players, the countdown, the end delay, returning players to
//! the hub and closing the instance.
//!
//! Gathering is a separate stage whose only output is the starting roster.
//! Today players gather inside the match instance; dedicated queue lobbies
//! can later gather them elsewhere and start a runner with that roster.

use super::{Destination, Game, GameEvent, Phase};
use crate::instance::{GameCtx, Transform};
use crate::lifecycle::PlayerRules;
use crate::simulation::SIMULATION_HZ;
use crate::text::{Text, Title};

#[derive(Clone, Copy, Debug)]
pub struct MatchRules {
    /// Players needed to start the countdown, and to keep it running.
    pub min_players: usize,
    /// Players admitted at once, including those still loading.
    pub max_players: usize,
    pub countdown_seconds: u64,
    /// Time between the result and returning everyone to the hub.
    pub end_seconds: u64,
}

/// How a round ended. The runner announces it.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub winners: Vec<i32>,
}

pub trait Match {
    fn kind(&self) -> &'static str;

    fn rules(&self) -> MatchRules;

    /// The instance starts, before anyone arrives: build terrain here.
    fn on_create(&mut self, _ctx: &mut GameCtx<'_>) {}

    /// Places a player arriving before the round starts, before its client
    /// learns its entity ID. `waiting` counts players here, it included.
    fn on_arrive(&mut self, ctx: &mut GameCtx<'_>, player: i32, waiting: usize) {
        let _ = waiting;
        let spawn = ctx.map_spawn();
        ctx.teleport(player, spawn);
        let at = ctx.position(player).unwrap_or(spawn);
        ctx.set_rules(player, PlayerRules::lobby(at));
    }

    /// The round begins with these players, in entity order.
    fn on_start(&mut self, ctx: &mut GameCtx<'_>, roster: &[i32]);

    /// Events while the round runs, including departures.
    fn on_event(&mut self, _ctx: &mut GameCtx<'_>, _event: GameEvent) {}

    /// One step of the round. Some ends it.
    fn tick(&mut self, ctx: &mut GameCtx<'_>) -> Option<Outcome>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Gathering,
    /// Steps left before the round starts.
    Countdown(u64),
    Running,
    /// Steps left before players return to the hub.
    Ending(u64),
    /// Players were sent away; the owner stops once the last one leaves.
    Closing,
}

pub struct MatchRunner<M> {
    game: M,
    rules: MatchRules,
    stage: Stage,
    /// Someone arrived, so an empty instance is abandoned rather than new.
    arrived: bool,
    /// Steps since the round started.
    elapsed: u64,
}

impl<M: Match> MatchRunner<M> {
    pub fn new(game: M) -> Self {
        let rules = game.rules();
        Self { game, rules, stage: Stage::Gathering, arrived: false, elapsed: 0 }
    }

    fn finish(&mut self, ctx: &mut GameCtx<'_>, outcome: Outcome) {
        let names: Vec<String> =
            outcome.winners.iter().filter_map(|&id| ctx.name(id)).map(str::to_owned).collect();
        let result = match names.as_slice() {
            [] => "Nobody wins.".to_owned(),
            [name] => format!("{name} wins!"),
            names => format!("{} win!", names.join(", ")),
        };
        ctx.broadcast(&result);
        // One line per round, so unattended games can be followed from the log.
        let seconds = self.elapsed / SIMULATION_HZ;
        println!("instance {} ({}): {result} ({seconds}s)", ctx.instance(), self.game.kind());
        self.stage = Stage::Ending(self.rules.end_seconds * SIMULATION_HZ);
    }
}

impl<M: Match> Game for MatchRunner<M> {
    fn kind(&self) -> &'static str {
        self.game.kind()
    }

    fn capacity(&self) -> usize {
        self.rules.max_players
    }

    fn phase(&self) -> Phase {
        match self.stage {
            Stage::Gathering | Stage::Countdown(_) => Phase::Open,
            Stage::Running => Phase::Running,
            Stage::Ending(_) | Stage::Closing => Phase::Ending,
        }
    }

    fn on_create(&mut self, ctx: &mut GameCtx<'_>) {
        self.game.on_create(ctx);
    }

    fn on_join(&mut self, ctx: &mut GameCtx<'_>, player: i32) {
        self.arrived = true;
        let waiting = ctx.player_count();
        self.game.on_arrive(ctx, player, waiting);
        let name = ctx.name(player).unwrap_or("?").to_owned();
        ctx.broadcast(format!("{name} joined ({waiting}/{}).", self.rules.max_players));
    }

    fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {
        if self.stage == Stage::Running {
            self.game.on_event(ctx, event);
        }
    }

    fn tick(&mut self, ctx: &mut GameCtx<'_>) {
        let count = ctx.player_count();
        let min = self.rules.min_players;
        self.stage = match self.stage {
            Stage::Closing => Stage::Closing,
            // Everyone left: there is nothing to finish or announce.
            _ if count == 0 && self.arrived => {
                ctx.close();
                Stage::Closing
            }
            Stage::Gathering if count >= min => {
                let seconds = self.rules.countdown_seconds;
                ctx.broadcast(format!("Starting in {seconds} seconds."));
                Stage::Countdown(seconds * SIMULATION_HZ)
            }
            Stage::Gathering => Stage::Gathering,
            Stage::Countdown(_) if count < min => {
                ctx.broadcast("Not enough players; waiting.");
                Stage::Gathering
            }
            Stage::Countdown(0) => {
                let roster: Vec<i32> = ctx.players().collect();
                self.game.on_start(ctx, &roster);
                Stage::Running
            }
            Stage::Countdown(steps) => {
                if steps % SIMULATION_HZ == 0 && steps <= 3 * SIMULATION_HZ {
                    let seconds = (steps / SIMULATION_HZ).to_string();
                    let title = Title::new(Text::new().gold(seconds)).timings(0, 25, 5);
                    ctx.broadcast_title(title);
                }
                Stage::Countdown(steps - 1)
            }
            Stage::Running => match self.game.tick(ctx) {
                Some(outcome) => {
                    self.finish(ctx, outcome);
                    self.stage
                }
                None => {
                    self.elapsed += 1;
                    Stage::Running
                }
            },
            Stage::Ending(0) => {
                let players: Vec<i32> = ctx.players().collect();
                for player in players {
                    ctx.send_to(player, Destination::Hub);
                }
                ctx.close();
                Stage::Closing
            }
            Stage::Ending(steps) => Stage::Ending(steps - 1),
        };
    }
}

/// A spawn on a ring around `center`, for `slot` of `slots` players.
pub fn ring_spawn(center: Transform, radius: f64, slot: usize, slots: usize) -> Transform {
    let angle = slot as f64 / slots.max(1) as f64 * std::f64::consts::TAU;
    Transform { x: center.x + radius * angle.cos(), z: center.z + radius * angle.sin(), ..center }
}
