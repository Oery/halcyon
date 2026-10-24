//! Bot ladder: one player against a sparring bot in a `boxes` box, with the
//! `boxes_long` kit. Beating the bot makes the next one stronger, losing
//! makes it weaker, and the next fight starts the moment one ends.
use std::cell::Cell;
use std::rc::Rc;

use halcyon_world::World;
use mc_protocol::types::ItemStack;

use super::boxes::{Kit, Layout};
use super::{Game, GameEvent, Phase};
use crate::instance::{GameCtx, Transform};
use crate::inventory::{HotbarSlot, InventorySlot};
use crate::lifecycle::PlayerRules;

/// Levels gained after a win, and lost after a loss.
const STEP: u8 = 10;
const START_LEVEL: u8 = 50;

/// What the ladder asks of its sparring bot. The instance manager starts a
/// bot task with it; the bot fights at `level` and takes a fresh fighter
/// whenever `generation` changes.
pub struct Sparring {
    pub name: String,
    pub level: Cell<u8>,
    pub generation: Cell<u32>,
    /// The player left; the bot leaves too.
    pub closed: Cell<bool>,
}

pub struct Ladder {
    layout: Layout,
    loadout: [ItemStack; InventorySlot::COUNT],
    sparring: Rc<Sparring>,
    player: Option<i32>,
    bot: Option<i32>,
    fighting: bool,
    round: u32,
    wins: u32,
    losses: u32,
    best: Option<u8>,
}

impl Ladder {
    pub const KIND: &str = "ladder";

    pub fn new(world: &World, name: String) -> Option<Self> {
        let level = std::env::var("HALCYON_LADDER_LEVEL")
            .ok()
            .and_then(|level| level.parse::<u8>().ok())
            .map_or(START_LEVEL, |level| level.min(100));
        Some(Self {
            layout: Layout::new(world)?,
            loadout: Kit::Long.loadout()?,
            sparring: Rc::new(Sparring {
                name,
                level: Cell::new(level),
                generation: Cell::new(0),
                closed: Cell::new(false),
            }),
            player: None,
            bot: None,
            fighting: false,
            round: 0,
            wins: 0,
            losses: 0,
            best: None,
        })
    }

    fn spawn(&self, player: i32) -> Transform {
        self.layout.spawn(0, usize::from(Some(player) == self.bot))
    }

    /// Both fighters at their starts with full health and the kit; the bot
    /// hears the start and engages.
    fn start_fight(&mut self, ctx: &mut GameCtx<'_>, fresh: &[i32]) {
        let (Some(player), Some(bot)) = (self.player, self.bot) else {
            return;
        };
        for id in [player, bot] {
            let spawn = self.spawn(id);
            ctx.set_rules(id, PlayerRules::open(spawn));
            // Before a reset: a fresh life drops actions queued for the old one.
            ctx.set_loadout(id, self.loadout.clone(), HotbarSlot::FIRST);
            if fresh.contains(&id) {
                ctx.reset(id);
            } else {
                ctx.teleport(id, spawn);
                ctx.heal(id, 20.0);
            }
        }
        self.round += 1;
        self.fighting = true;
        self.sparring.generation.set(self.sparring.generation.get() + 1);
        let level = self.sparring.level.get();
        ctx.message(player, &format!("Round {}: a level {level} bot.", self.round));
        ctx.broadcast(super::FIGHT);
    }

    fn finish(&mut self, ctx: &mut GameCtx<'_>, loser: i32) {
        let Some(player) = self.player else { return };
        self.fighting = false;
        let level = self.sparring.level.get();
        let won = loser != player;
        let next = if won {
            self.wins += 1;
            self.best = self.best.max(Some(level));
            level.saturating_add(STEP).min(100)
        } else {
            self.losses += 1;
            level.saturating_sub(STEP)
        };
        self.sparring.level.set(next);
        let record = format!("{}-{}", self.wins, self.losses);
        let best = self.best.map_or(String::new(), |best| format!(", best win: level {best}"));
        let result = if won { "You win" } else { "You lose" };
        ctx.message(player, &format!("{result} ({record}{best}). Next: level {next}."));
        self.start_fight(ctx, &[loser]);
    }
}

impl Game for Ladder {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    fn capacity(&self) -> usize {
        2
    }

    /// Open until a player arrives; the box is theirs alone.
    fn phase(&self) -> Phase {
        if self.player.is_some() {
            Phase::Running
        } else {
            Phase::Open
        }
    }

    fn sparring(&self) -> Option<Rc<Sparring>> {
        Some(self.sparring.clone())
    }

    fn on_create(&mut self, ctx: &mut GameCtx<'_>) {
        if let Err(error) = ctx.set_blocks(&self.layout.cells) {
            eprintln!("instance {} (ladder): arena build failed: {error:?}", ctx.instance());
        }
    }

    fn on_join(&mut self, ctx: &mut GameCtx<'_>, id: i32) {
        if ctx.name(id) == Some(self.sparring.name.as_str()) {
            self.bot = Some(id);
        } else {
            self.player = Some(id);
        }
        let spawn = self.spawn(id);
        ctx.teleport(id, spawn);
        ctx.set_rules(id, PlayerRules::lobby(spawn));
    }

    fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {
        match event {
            GameEvent::Died { player, .. } if self.fighting => self.finish(ctx, player),
            GameEvent::Left(id) if Some(id) == self.player || Some(id) == self.bot => {
                self.sparring.closed.set(true);
                ctx.close();
            }
            _ => {}
        }
    }

    fn tick(&mut self, ctx: &mut GameCtx<'_>) {
        if self.fighting {
            return;
        }
        let ready = |id: Option<i32>| id.is_some_and(|id| ctx.alive(id));
        if ready(self.player) && ready(self.bot) {
            let player = self.player.unwrap_or_default();
            ctx.message(
                player,
                "Bot ladder: beat a bot to face a stronger one; lose and the next is weaker.",
            );
            self.start_fight(ctx, &[]);
        }
    }
}
