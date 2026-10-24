//! Melee combat brain. A fighter picks the nearest opponent it can reach and
//! reacts to what a client shows: positions a few ticks late, hurt flashes
//! and arm swings. It plays modern sword PvP (see `docs/minecraft-combat.md`):
//! swings timed to the strong-attack point, spacing at the edge of reach while
//! recharging, sprint resets, crits, jump resets, hit-selecting, combos,
//! evasion and retreats. `Tactics` decides which of these a bot uses and how
//! precisely, from its level and traits. Like the parkour brain, it acts only
//! through `Motor` inputs and the attacks a client would send.

use std::collections::VecDeque;

use halcyon_world::{Aabb, BlockView};

use super::motor::{Intent, Motor, wrap_degrees};
use super::profile::BotProfile;
use super::rng::Rng;
use crate::instance::{Cues, Transform};
use crate::movement::MovementInput;

/// Standing eye height.
pub const EYE: f64 = 1.62;
const HALF_WIDTH: f64 = 0.3;
const HEIGHT: f64 = 1.8;
/// Vanilla's entity interaction range; the bot clicks only within it.
const REACH: f64 = 3.0;
/// Opponents farther than this are ignored.
const SIGHT: f64 = 24.0;
/// Positions remembered per target, which bounds the reaction delay.
const HISTORY: usize = 12;
/// A hurt player ignores weaker hits for this many ticks.
const DAMAGE_WINDOW: u64 = 10;
/// Charge above which an attack is strong (`combat::strong`).
const STRONG: f32 = 0.9;
/// Ticks an opponent needs between swings, assuming a sword.
const OPPONENT_RECHARGE: u64 = 11;
/// Horizontal ground speed while sprinting, in blocks per tick.
const SPRINT_SPEED: f64 = 0.28;

/// Another player as a client would see it.
#[derive(Clone, Copy, Debug)]
pub struct Opponent {
    pub id: i32,
    pub transform: Transform,
    /// When it last flashed hurt or swung its arm.
    pub cues: Cues,
}

/// Everything a fighter perceives on one tick.
#[derive(Clone, Copy, Debug)]
pub struct Senses<'a> {
    /// The instance's simulation step, which cues are stamped with.
    pub tick: u64,
    pub me: Transform,
    /// This bot's own health, as its HUD shows it.
    pub health: f32,
    /// This bot was hurt since its last decision.
    pub hurt: bool,
    /// The held weapon's full-charge time in ticks.
    pub delay: f32,
    pub opponents: &'a [Opponent],
}

#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub input: MovementInput,
    /// Swing the arm this tick, at `attack` or at nothing.
    pub swing: bool,
    /// The player under the crosshair when swinging.
    pub attack: Option<i32>,
}

/// What a bot does right after a sprint-knockback hit. Every option is
/// open to the bench's tuner, even those `MASTER` does not use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum SprintReset {
    /// Keep holding forward and sprint.
    Hold,
    /// Release forward for a few ticks, then sprint again.
    WTap,
    /// Tap backward for a few ticks to rebuild spacing.
    STap,
    /// S-tap when the target stays close, hold when chasing a combo, else w-tap.
    Adaptive,
}

/// Which techniques a fighter uses, and how well. Fixed at creation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tactics {
    /// The first version's behavior, kept as the bench's baseline.
    pub basic: bool,
    /// Spread of the click around the strong-attack tick, in ticks.
    pub timing_sd: f32,
    /// Chance of holding a click while the target's damage window is open.
    pub respect_iframes: f32,
    /// Hover outside reach while recharging instead of closing in.
    pub spacing: bool,
    /// Gap kept while recharging behind the opponent, eye to body, in blocks.
    pub hover: f64,
    /// Gap held when charged first, waiting to step in.
    pub wait_gap: f64,
    /// Ticks of slack when timing a step-in to the charge.
    pub arrive_slack: f64,
    pub sprint_reset: SprintReset,
    /// Longest w-tap or s-tap, in ticks.
    pub tap_ticks: u32,
    /// Below this gap after a hit, the adaptive reset s-taps.
    pub stap_gap: f64,
    /// Chance per tick of jumping in for a critical when a window opens.
    pub crits: f32,
    /// Ticks before the planned click a critical jump starts.
    pub crit_lead: i32,
    /// Chance of jumping as a hit lands, to cut its knockback.
    pub jump_reset: f32,
    /// Chance of letting a charged opponent swing first, then hitting back.
    pub hit_select: f32,
    /// Chase knocked-back targets to arrive charged.
    pub combo: bool,
    /// Receding speed that starts a chase, in blocks per tick.
    pub chase_speed: f64,
    /// Chance of dodging sideways when a charged opponent swings.
    pub evasion: f32,
    /// Back off to recharge after taking a combo or at low health.
    pub retreat: bool,
    pub retreat_health: f32,
    /// Hits taken in a row that call for a retreat.
    pub retreat_streak: u32,
    /// Strafe direction holds, in ticks, and the chance of not strafing.
    pub strafe_min: u32,
    pub strafe_max: u32,
    pub strafe_still: f32,
    /// Ticks between the crosshair meeting the target and the click.
    pub click_lag: f32,
    /// Fraction of a moving target's motion over the perception lag that the
    /// aim makes up for, to aim where the client shows the target now.
    pub lead: f32,
    /// Fraction of the aim point's sweep across the view the camera pursues.
    pub pursuit: f32,
    /// Trait: willingness to close in early and trade.
    pub aggression: f32,
}

/// A strong player's tactics, found with the combat bench's tuner
/// (`tune_tactics`). Lower levels scale down from these.
pub const MASTER: Tactics = Tactics {
    basic: false,
    timing_sd: 0.5,
    respect_iframes: 0.95,
    spacing: false,
    hover: 3.6,
    wait_gap: 3.15,
    arrive_slack: 1.0,
    sprint_reset: SprintReset::Adaptive,
    tap_ticks: 3,
    stap_gap: 2.2,
    crits: 0.5,
    crit_lead: 8,
    jump_reset: 0.85,
    hit_select: 0.0,
    combo: true,
    chase_speed: 0.08,
    evasion: 0.7,
    retreat: false,
    retreat_health: 6.0,
    retreat_streak: 2,
    strafe_min: 8,
    strafe_max: 30,
    strafe_still: 0.15,
    click_lag: 0.7,
    lead: 1.0,
    pursuit: 1.0,
    aggression: 0.5,
};

impl Tactics {
    #[cfg(test)]
    pub fn basic() -> Self {
        Self {
            basic: true,
            timing_sd: 0.0,
            respect_iframes: 0.0,
            spacing: false,
            hit_select: 0.0,
            combo: false,
            evasion: 0.0,
            retreat: false,
            crits: 0.0,
            jump_reset: 0.0,
            lead: 0.0,
            pursuit: 0.0,
            ..MASTER
        }
    }

    /// Techniques and their precision for a level; `aggression` and
    /// `patience` are traits in `0..=1`, drawn once per bot.
    pub fn for_level(profile: BotProfile, aggression: f32, patience: f32) -> Self {
        Self::scaled(MASTER, profile, aggression, patience)
    }

    /// `master` scaled down to `profile`'s level: a beginner barely uses
    /// the techniques, and uses them sloppily.
    pub fn scaled(master: Self, profile: BotProfile, aggression: f32, patience: f32) -> Self {
        let skill = f32::from(profile.level) / 100.0;
        let learned = |from: f32| ((skill - from) / (1.0 - from)).clamp(0.0, 1.0);
        Self {
            basic: false,
            timing_sd: 2.5 - (2.5 - master.timing_sd) * skill,
            respect_iframes: master.respect_iframes * (0.3 + 0.7 * skill),
            // Patient players keep their distance and back off to recharge.
            spacing: (master.spacing || patience > 0.65) && skill >= 0.3,
            hover: master.hover + 0.35 * f64::from(patience - 0.5)
                - 0.2 * f64::from(aggression - 0.5),
            sprint_reset: if skill >= 0.4 {
                master.sprint_reset
            } else {
                SprintReset::WTap
            },
            crits: master.crits * (0.2 + 0.8 * learned(0.2)),
            jump_reset: master.jump_reset * learned(0.3),
            hit_select: master.hit_select * learned(0.5) * (0.5 + patience),
            combo: master.combo && skill >= 0.25,
            evasion: master.evasion * learned(0.3),
            retreat: (master.retreat || patience > 0.75) && skill >= 0.35,
            click_lag: master.click_lag + 2.5 * (1.0 - skill),
            lead: master.lead * (0.6 + 0.4 * skill),
            pursuit: 0.4 + (master.pursuit - 0.4) * skill,
            aggression,
            ..master
        }
    }

    /// `full` with one technique switched off at a time, for the bench.
    #[cfg(test)]
    pub fn ablations(full: Self) -> Vec<(&'static str, Self)> {
        vec![
            ("no-timing", Self { timing_sd: 2.5, ..full }),
            ("no-iframes", Self { respect_iframes: 0.0, ..full }),
            ("spacing", Self { spacing: true, ..full }),
            ("hold-sprint", Self { sprint_reset: SprintReset::Hold, ..full }),
            ("wtap-only", Self { sprint_reset: SprintReset::WTap, ..full }),
            ("stap-only", Self { sprint_reset: SprintReset::STap, ..full }),
            ("no-crits", Self { crits: 0.0, ..full }),
            ("no-jump-reset", Self { jump_reset: 0.0, ..full }),
            ("hit-select", Self { hit_select: 0.8, ..full }),
            ("no-combo", Self { combo: false, ..full }),
            ("no-evasion", Self { evasion: 0.0, ..full }),
            ("retreat", Self { retreat: true, ..full }),
            ("no-lead", Self { lead: 0.0, ..full }),
            ("no-pursuit", Self { pursuit: 0.0, ..full }),
        ]
    }
}

/// How often a fighter tried each technique, for the bench.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub crit_plans: u32,
    /// Swings on the way down of a critical jump.
    pub crit_swings: u32,
    /// Critical jumps that landed without a swing.
    pub crit_landed: u32,
    pub jump_resets: u32,
    pub dodges: u32,
    pub retreats: u32,
    pub holds: u32,
    pub whiffs: u32,
}

/// A short committed movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Maneuver {
    None,
    /// Forward released after a hit.
    WTap(u32),
    /// Backward tapped after a hit.
    STap(u32),
    /// Backing off to recharge.
    Retreat(u32),
    /// Sideways out of an incoming swing.
    Dodge(u32),
}

pub struct Fighter {
    profile: BotProfile,
    tactics: Tactics,
    rng: Rng,
    motor: Motor,
    /// Whether the fight is on: set by the round's start, cleared by dying.
    pub engaged: bool,
    pub counters: Counters,
    target: Option<i32>,
    /// The target's positions, oldest first; the bot reacts to a stale one.
    seen: VecDeque<Transform>,
    /// Ticks since this bot's last swing, as the server counts its charge.
    since_swing: u32,
    /// Charge to wait for before the next swing (basic tactics).
    threshold: f32,
    /// Ticks after the strong-attack point for the next click; negative is early.
    click_offset: i32,
    /// -1 strafes left, 1 right, 0 holds the line.
    strafe: i8,
    strafe_for: u32,
    maneuver: Maneuver,
    /// Ticks left with forward released after a hit (basic tactics).
    wtap: u32,
    /// Jumped in for a critical and still airborne.
    crit_jump: bool,
    /// Holding a charged swing for the opponent to commit first.
    held_for: u32,
    /// Consecutive hits taken without landing one, and landed without taking one.
    taken: u32,
    landed: u32,
    /// The last swing's tick, to credit the target's next hurt flash to it.
    swung_at: Option<u64>,
    last_me: Option<Transform>,
    /// This decision's reaction delay, in ticks.
    lag: u32,
    /// Whether the crosshair was on the rendered target, by tick.
    on_history: VecDeque<bool>,
    /// Degrees between the crosshair and the target's chest, for traces.
    aim_error: f64,
    /// Gap, crosshair on the rendered target, and wanting to click, for traces.
    trace: (f64, bool, bool),
    /// The previous aim direction, for smooth pursuit.
    last_aim: Option<(f32, f32)>,
    /// A per-bot aim point on the target's body, in blocks above its feet.
    aim_height: f64,
    /// Idle head turns when there is nobody to fight.
    idle_yaw: f32,
    idle_for: u32,
}

impl Fighter {
    /// A fighter with tactics from its level and traits drawn from `seed`.
    pub fn new(profile: BotProfile, seed: u64, facing: Transform) -> Self {
        let mut traits = Rng::new(seed ^ 0x07ac_71c5);
        let (aggression, patience) = (traits.unit() as f32, traits.unit() as f32);
        Self::with_tactics(profile, Tactics::for_level(profile, aggression, patience), seed, facing)
    }

    pub fn with_tactics(
        profile: BotProfile,
        tactics: Tactics,
        seed: u64,
        facing: Transform,
    ) -> Self {
        let mut rng = Rng::new(seed);
        let threshold = profile.click_threshold(&mut rng);
        let aim_height = rng.range(1.0, 1.45);
        Self {
            profile,
            tactics,
            rng,
            motor: Motor::new(facing.yaw, facing.pitch),
            engaged: false,
            counters: Counters::default(),
            target: None,
            seen: VecDeque::with_capacity(HISTORY),
            since_swing: 100,
            threshold,
            click_offset: 0,
            strafe: 0,
            strafe_for: 0,
            maneuver: Maneuver::None,
            wtap: 0,
            crit_jump: false,
            held_for: 0,
            taken: 0,
            landed: 0,
            swung_at: None,
            last_me: None,
            last_aim: None,
            lag: profile.reaction_ticks(),
            on_history: VecDeque::with_capacity(HISTORY),
            aim_error: 0.0,
            trace: (0.0, false, false),
            aim_height,
            idle_yaw: facing.yaw,
            idle_for: 0,
        }
    }

    /// Whether it is fighting someone right now.
    #[cfg(test)]
    pub fn engaged_target(&self) -> bool {
        self.target.is_some()
    }

    /// A one-line state summary for traces.
    #[cfg(test)]
    pub fn debug_state(&self) -> String {
        format!(
            "since={} err={:.1} gap={:.2} on={} wants={} offset={} maneuver={:?} crit={} held={} taken={} landed={}",
            self.since_swing,
            self.aim_error,
            self.trace.0,
            self.trace.1,
            self.trace.2,
            self.click_offset,
            self.maneuver,
            self.crit_jump,
            self.held_for,
            self.taken,
            self.landed
        )
    }

    /// One decision.
    pub fn tick(&mut self, senses: &Senses<'_>, view: &impl BlockView) -> Decision {
        self.since_swing = self.since_swing.saturating_add(1);
        let Some(target) = self.choose(senses.me, senses.opponents, view) else {
            self.last_me = Some(senses.me);
            return self.idle(senses.me, senses.opponents);
        };
        // React to where the target was a moment ago.
        if self.seen.len() == HISTORY {
            self.seen.pop_front();
        }
        self.seen.push_back(target.transform);
        self.lag = if self.tactics.basic {
            self.profile.reaction_ticks()
        } else {
            self.draw(self.profile.reaction())
        };
        let decision = if self.tactics.basic {
            self.tick_basic(senses, target, view)
        } else {
            self.tick_advanced(senses, target, view)
        };
        self.last_me = Some(senses.me);
        decision
    }

    /// Whole ticks around a mean: its floor, or the next tick with the
    /// fraction's chance.
    fn draw(&mut self, mean: f32) -> u32 {
        let floor = mean.max(0.0).floor();
        floor as u32 + u32::from(self.rng.chance(f64::from(mean - floor)))
    }

    /// The perceived target `back` ticks before the reaction-delayed one.
    fn perceived(&self, back: usize) -> Transform {
        let lag = self.lag as usize + back;
        self.seen[self.seen.len() - 1 - lag.min(self.seen.len() - 1)]
    }

    fn tick_basic(
        &mut self,
        senses: &Senses<'_>,
        target: Opponent,
        view: &impl BlockView,
    ) -> Decision {
        let (me, delay) = (senses.me, senses.delay);
        let eye = [me.x, me.y + EYE, me.z];
        let seen = self.perceived(0);
        let (yaw, pitch) = look_at(eye, [seen.x, seen.y + 1.2, seen.z]);
        let distance = (seen.x - me.x).hypot(seen.z - me.z);

        if self.strafe_for == 0 {
            self.strafe = [-1, 0, 1, 1, -1][self.rng.ticks(0, 4) as usize];
            self.strafe_for = self.rng.ticks(10, 40);
        }
        self.strafe_for -= 1;
        let charge = (self.since_swing as f32 + 0.5) / delay.max(1.0);
        let close = distance < 4.0;
        let mut intent = Intent {
            yaw,
            pitch,
            forward: distance > 1.8 && self.wtap == 0,
            backward: distance < 1.2,
            left: close && self.strafe < 0,
            right: close && self.strafe > 0,
            sprint: distance > 2.2 && self.wtap == 0,
            precise: close,
            ..Intent::default()
        };
        self.wtap = self.wtap.saturating_sub(1);
        // A jump on the way in lands a critical as the charge comes back.
        if me.on_ground
            && (2.4..4.0).contains(&distance)
            && charge > 0.7
            && self.rng.chance(self.profile.crit_jump_chance() / 6.0)
        {
            intent.jump = true;
        }
        let input = self.motor.tick(intent, self.profile, &mut self.rng);

        // Click only with the crosshair on the target as it stands now.
        let on_target = aimed_at(eye, input.yaw, input.pitch, target.transform, view);
        let ready = charge >= self.threshold
            || (charge > 0.3 && self.rng.chance(self.profile.spam_chance()));
        let attack = (on_target && ready).then_some(target.id);
        if attack.is_some() {
            self.since_swing = 0;
            self.threshold = self.profile.click_threshold(&mut self.rng);
            if input.sprinting && self.rng.chance(self.profile.wtap_chance()) {
                self.wtap = self.rng.ticks(2, 4);
            }
        }
        Decision { input, swing: attack.is_some(), attack }
    }

    fn tick_advanced(
        &mut self,
        senses: &Senses<'_>,
        target: Opponent,
        view: &impl BlockView,
    ) -> Decision {
        let t = self.tactics;
        let (me, tick) = (senses.me, senses.tick);
        let eye = [me.x, me.y + EYE, me.z];

        // Exchange bookkeeping from the cues: whose hits are landing.
        let target_hurt_age = target.cues.hurt_at.map_or(u64::MAX, |at| tick.saturating_sub(at));
        if target_hurt_age == 0 && self.swung_at.is_some_and(|at| tick - at <= 2) {
            self.landed += 1;
            self.taken = 0;
        }
        if senses.hurt {
            self.taken += 1;
            self.landed = 0;
        }

        // Where the target is, as perceived, and where it is heading.
        let seen = self.perceived(0);
        let before = self.perceived(1);
        let velocity = [seen.x - before.x, seen.z - before.z];
        let rise = seen.y - before.y;
        // Track by prediction: extrapolate from what was perceived a reaction
        // ago to the frame the client shows now (one tick old), and pursue
        // the aim point's motion.
        let lead = f64::from(t.lead) * f64::from(self.lag.saturating_sub(1));
        let aim = [
            seen.x + velocity[0] * lead,
            seen.y + rise * lead + self.aim_height,
            seen.z + velocity[1] * lead,
        ];
        let (yaw, pitch) = look_at(eye, aim);
        let (yaw_rate, pitch_rate) = self.last_aim.map_or((0.0, 0.0), |(last_yaw, last_pitch)| {
            let pursuit = t.pursuit.clamp(0.0, 1.0);
            (
                (wrap_degrees(yaw - last_yaw) * pursuit).clamp(-30.0, 30.0),
                (pitch - last_pitch) * pursuit,
            )
        });
        self.last_aim = Some((yaw, pitch));
        let gap = crate::combat::distance_sqr(player_bounds(seen), eye).sqrt();
        let my_velocity =
            self.last_me.map_or([0.0; 3], |last| [me.x - last.x, me.y - last.y, me.z - last.z]);
        let toward = [seen.x - me.x, seen.z - me.z];
        let horizontal = toward[0].hypot(toward[1]).max(1e-6);
        // Positive when the target moves away from this bot.
        let receding = (velocity[0] * toward[0] + velocity[1] * toward[1]) / horizontal;

        // Charge: the first strong tick, and when this bot plans to click.
        // The server counts the charge from the tick after this bot's click.
        let strong_at = (STRONG * senses.delay - 0.5).ceil().max(0.0) as i32 + 1;
        let until_strong = strong_at - self.since_swing as i32;
        let until_click = until_strong + self.click_offset;
        let opponent_swing_age =
            target.cues.swing_at.map_or(u64::MAX, |at| tick.saturating_sub(at));
        let opponent_ready = opponent_swing_age >= OPPONENT_RECHARGE;
        let opponent_recharge = (OPPONENT_RECHARGE + 1).saturating_sub(opponent_swing_age) as i32;
        let threatened = opponent_ready && gap < REACH + 0.4;
        let shielded = target_hurt_age < DAMAGE_WINDOW;

        // Jump reset: jump as the hit lands, while still on the ground, so
        // the sprint-jump boost toward the attacker cancels knockback.
        let jump_reset = senses.hurt && me.on_ground && self.rng.chance(f64::from(t.jump_reset));

        // Retreat after a lost exchange or at low health, to recharge.
        if t.retreat
            && matches!(self.maneuver, Maneuver::None)
            && until_strong > 3
            && threatened
            && (self.taken >= t.retreat_streak || senses.health < t.retreat_health)
            && self.rng.chance(0.5)
        {
            self.maneuver = Maneuver::Retreat(self.rng.ticks(6, 12));
            self.counters.retreats += 1;
        }
        // Dodge sideways from a charged opponent's swing when not ready to trade.
        if opponent_swing_age == 0
            && gap < REACH + 0.6
            && until_strong > 2
            && matches!(self.maneuver, Maneuver::None)
            && self.rng.chance(f64::from(t.evasion))
        {
            self.strafe = -self.strafe.signum().max(1) * if self.rng.chance(0.5) { 1 } else { -1 };
            self.maneuver = Maneuver::Dodge(self.rng.ticks(3, 6));
            self.counters.dodges += 1;
        }

        // Strafing: humanlike direction changes, away from walls.
        if self.strafe_for == 0 {
            self.strafe = if self.rng.chance(f64::from(t.strafe_still)) {
                0
            } else if self.rng.chance(0.5) {
                1
            } else {
                -1
            };
            self.strafe_for = self.rng.ticks(t.strafe_min, t.strafe_max);
        }
        self.strafe_for -= 1;
        let side_blocked = |side: i8| side != 0 && wall_near(view, me, yaw, side, 1.3);
        if side_blocked(self.strafe) {
            self.strafe = -self.strafe;
            self.strafe_for = self.rng.ticks(8, 20);
        }
        // Backed against a wall there is no room to give: never back up
        // further, and slip out along it toward the side with room.
        let back_blocked = wall_behind(view, me, yaw, 1.8);
        if back_blocked {
            let room = |side: i8| !wall_near(view, me, yaw, side, 2.5);
            let current = if self.strafe == 0 { 1 } else { self.strafe };
            self.strafe = if room(current) || !room(-current) {
                current
            } else {
                -current
            };
            self.strafe_for = self.strafe_for.max(6);
        }

        // Movement: close in to arrive as the charge comes back, hover outside
        // reach while recharging, chase knocked-back targets.
        let arrival = ((gap - (REACH - 0.15)) / SPRINT_SPEED).max(0.0);
        let charged_soon = f64::from(until_click.max(0)) <= arrival + t.arrive_slack;
        let chasing = t.combo && receding > t.chase_speed && target_hurt_age < 12;
        let mut forward;
        let mut backward;
        let mut sprint;
        if chasing || (charged_soon && t.spacing) {
            // Sprint in: the hit lands at the edge of reach, sprinting, but
            // never into the opponent's body.
            forward = gap > 1.6;
            sprint = forward;
            backward = gap < 0.9 && !back_blocked;
        } else if !t.spacing {
            forward = gap > 1.0;
            sprint = gap > 1.6;
            backward = gap < 0.6;
        } else if until_click <= opponent_recharge {
            // Charged first: wait just outside reach to step in.
            forward = gap > t.wait_gap;
            sprint = gap > t.wait_gap + 0.65;
            backward = gap < REACH - 0.5 && !back_blocked;
        } else {
            // Recharging behind the opponent: stay out of its reach.
            forward = gap > t.hover + 0.3;
            sprint = gap > t.hover + 1.0;
            backward = gap < t.hover - 0.25 && !back_blocked;
        }
        let mut strafe = if gap < 5.0 && !chasing { self.strafe } else { 0 };
        match self.maneuver {
            Maneuver::WTap(_) => {
                forward = false;
                sprint = false;
            }
            Maneuver::STap(_) => {
                forward = false;
                sprint = false;
                backward = !back_blocked;
            }
            Maneuver::Retreat(_) => {
                forward = false;
                sprint = false;
                backward = !back_blocked;
                if back_blocked && strafe == 0 {
                    strafe = if self.rng.chance(0.5) { 1 } else { -1 };
                }
            }
            Maneuver::Dodge(_) => {
                strafe = if self.strafe == 0 { 1 } else { self.strafe };
            }
            Maneuver::None => {}
        }
        if back_blocked && !chasing {
            // Diagonally forward past the opponent, out of the corner.
            if strafe == 0 {
                strafe = self.strafe.signum().max(1);
            }
            forward = gap > 1.0;
            backward = false;
        }
        self.maneuver = match self.maneuver {
            Maneuver::WTap(n) | Maneuver::STap(n) | Maneuver::Retreat(n) | Maneuver::Dodge(n)
                if n <= 1 =>
            {
                Maneuver::None
            }
            Maneuver::WTap(n) => Maneuver::WTap(n - 1),
            Maneuver::STap(n) => Maneuver::STap(n - 1),
            Maneuver::Retreat(n) => Maneuver::Retreat(n - 1),
            Maneuver::Dodge(n) => Maneuver::Dodge(n - 1),
            Maneuver::None => Maneuver::None,
        };

        // Crits: jump on the way in so the bot is falling, sprint released,
        // when the charge comes back.
        let mut jump = jump_reset;
        if jump_reset {
            forward = true;
            sprint = true;
            backward = false;
        }
        if me.on_ground {
            if self.crit_jump {
                self.counters.crit_landed += 1;
            }
            self.crit_jump = false;
        }
        self.counters.jump_resets += u32::from(jump_reset);
        // A jump reset whose descent (six to ten ticks on) covers the planned
        // click doubles as a critical jump: possible when the hit landed
        // mid-charge, not right after a trade.
        if jump_reset && (6..=9).contains(&until_click) && self.rng.chance(f64::from(t.crits)) {
            self.crit_jump = true;
            self.counters.crit_plans += 1;
        }
        // A jump rises for six ticks and falls for six; jump so the planned
        // click comes on the way down, from where the run-in closes to reach.
        let closing = (SPRINT_SPEED - receding).max(0.05);
        let gap_at_click = gap - closing * f64::from(until_click.max(0));
        let crit_window = (t.crit_lead - 1..=t.crit_lead + 1).contains(&until_click)
            && (1.2..REACH).contains(&gap_at_click);
        if !jump
            && me.on_ground
            && crit_window
            && (chasing || !threatened || self.landed > 0)
            && self.rng.chance(f64::from(t.crits))
        {
            jump = true;
            self.crit_jump = true;
            self.counters.crit_plans += 1;
            forward = true;
            sprint = true;
            backward = false;
        }
        let falling = !me.on_ground && my_velocity[1] < 0.0;
        if self.crit_jump {
            // Keep the run-in's momentum, but a critical needs sprint released.
            forward = gap > 1.4;
            sprint = !falling && forward;
            backward = false;
        }

        let intent = Intent {
            yaw,
            pitch,
            forward,
            backward: backward && !forward,
            left: strafe < 0,
            right: strafe > 0,
            jump,
            sprint: sprint && forward,
            precise: gap < 4.5,
            yaw_rate,
            pitch_rate,
            ..Intent::default()
        };
        let input = self.motor.tick(intent, self.profile, &mut self.rng);

        // Clicks: the crosshair on the target as the client renders it, a
        // tick late, noticed after this bot's click reaction, at the planned
        // charge, and not into an open damage window unless the hit is a crit.
        let rendered = self.seen[self.seen.len().saturating_sub(2)];
        let on_target = aimed_at(eye, input.yaw, input.pitch, rendered, view);
        if self.on_history.len() == HISTORY {
            self.on_history.pop_front();
        }
        self.on_history.push_back(on_target);
        // The click answers what the crosshair showed `click_lag` ticks ago;
        // it hits whatever is under it now, possibly nothing.
        let noticed = self
            .on_history
            .len()
            .checked_sub(1 + self.draw(t.click_lag) as usize)
            .is_some_and(|index| self.on_history[index]);
        let mut wants = until_click <= 0;
        // Save the swing for the descent, unless a charged opponent is about
        // to hit first.
        if self.crit_jump && !falling && !(threatened && opponent_swing_age > 0) {
            wants = false;
        }
        if shielded && !(self.crit_jump && falling) && self.rng.chance(f64::from(t.respect_iframes))
        {
            wants = false;
        }
        // Hit-select: let a charged, closing opponent swing first.
        let trading =
            opponent_ready && until_strong <= 0 && (REACH - 0.4..REACH + 0.3).contains(&gap);
        if wants
            && trading
            && !senses.hurt
            && opponent_swing_age > 0
            && self.held_for < 6
            && (self.held_for > 0 || self.rng.chance(f64::from(t.hit_select)))
        {
            self.held_for += 1;
            self.counters.holds += 1;
            wants = false;
        }
        self.aim_error = angle_to(eye, input.yaw, input.pitch, target.transform);
        self.trace = (gap, on_target, wants);
        let swing = noticed && wants;
        let attack = (swing && on_target).then_some(target.id);
        if swing {
            self.counters.whiffs += u32::from(attack.is_none());
            if self.crit_jump && falling {
                self.counters.crit_swings += 1;
                // The plan is spent; landing now is no abort.
                self.crit_jump = false;
            }
            self.since_swing = 0;
            self.held_for = 0;
            self.swung_at = Some(tick);
            self.click_offset =
                (self.rng.normal() as f32 * t.timing_sd).round().clamp(-3.0, 5.0) as i32;
            if attack.is_some() && input.sprinting && matches!(self.maneuver, Maneuver::None) {
                self.maneuver = self.sprint_reset(gap, chasing);
            }
        } else if !trading {
            self.held_for = 0;
        }
        Decision { input, swing, attack }
    }

    fn sprint_reset(&mut self, gap: f64, chasing: bool) -> Maneuver {
        let ticks = self.rng.ticks(1, self.tactics.tap_ticks.max(1));
        match self.tactics.sprint_reset {
            SprintReset::Hold => Maneuver::None,
            SprintReset::WTap => Maneuver::WTap(ticks),
            SprintReset::STap => Maneuver::STap(ticks),
            SprintReset::Adaptive if gap < self.tactics.stap_gap => Maneuver::STap(ticks),
            SprintReset::Adaptive if chasing => Maneuver::None,
            SprintReset::Adaptive => Maneuver::WTap(ticks),
        }
    }

    /// The current target if still reachable, else the nearest reachable opponent.
    fn choose(
        &mut self,
        me: Transform,
        opponents: &[Opponent],
        view: &impl BlockView,
    ) -> Option<Opponent> {
        if !self.engaged {
            self.target = None;
            return None;
        }
        let reachable = |o: &&Opponent| {
            let t = o.transform;
            (t.x - me.x).hypot(t.z - me.z) < SIGHT && clear_path(view, me, t)
        };
        let current = self.target.and_then(|id| opponents.iter().find(|o| o.id == id));
        let chosen = current.filter(reachable).or_else(|| {
            opponents.iter().filter(reachable).min_by(|a, b| {
                distance_sqr(me, a.transform).total_cmp(&distance_sqr(me, b.transform))
            })
        })?;
        if self.target != Some(chosen.id) {
            self.target = Some(chosen.id);
            self.seen.clear();
            self.on_history.clear();
            self.last_aim = None;
            self.taken = 0;
            self.landed = 0;
        }
        Some(*chosen)
    }

    /// Stand and look around, now and then toward the nearest player.
    fn idle(&mut self, me: Transform, opponents: &[Opponent]) -> Decision {
        self.target = None;
        if self.idle_for == 0 {
            self.idle_for = self.rng.ticks(20, 80);
            let nearest = opponents.iter().min_by(|a, b| {
                distance_sqr(me, a.transform).total_cmp(&distance_sqr(me, b.transform))
            });
            self.idle_yaw = match nearest {
                Some(o) if self.rng.chance(0.6) => {
                    let eye = [me.x, me.y + EYE, me.z];
                    look_at(eye, [o.transform.x, o.transform.y + EYE, o.transform.z]).0
                }
                _ => self.idle_yaw + self.rng.range(-70.0, 70.0) as f32,
            };
        }
        self.idle_for -= 1;
        let intent = Intent { yaw: self.idle_yaw, pitch: 0.0, ..Intent::default() };
        let input = self.motor.tick(intent, self.profile, &mut self.rng);
        Decision { input, swing: false, attack: None }
    }
}

/// A solid block within `distance` to one side (`side` -1 left, 1 right) of
/// a player facing `yaw`, at chest height.
fn wall_near(view: &impl BlockView, me: Transform, yaw: f32, side: i8, distance: f64) -> bool {
    // Minecraft's left is +90 degrees of yaw from forward.
    let direction = look_direction(yaw - 90.0 * f32::from(side), 0.0);
    probe(view, me, direction, distance)
}

fn wall_behind(view: &impl BlockView, me: Transform, yaw: f32, distance: f64) -> bool {
    probe(view, me, look_direction(yaw + 180.0, 0.0), distance)
}

fn probe(view: &impl BlockView, me: Transform, direction: [f64; 3], distance: f64) -> bool {
    let from = [me.x, me.y + 1.0, me.z];
    let to = [from[0] + direction[0] * distance, from[1], from[2] + direction[2] * distance];
    blocked(view, from, to)
}

/// Degrees between a camera's view and the direction to `target`'s chest.
fn angle_to(eye: [f64; 3], yaw: f32, pitch: f32, target: Transform) -> f64 {
    let look = look_direction(yaw, pitch);
    let to = [target.x - eye[0], target.y + 1.2 - eye[1], target.z - eye[2]];
    let length = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt().max(1e-9);
    let cos = (look[0] * to[0] + look[1] * to[1] + look[2] * to[2]) / length;
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

fn distance_sqr(a: Transform, b: Transform) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)
}

/// Yaw and pitch from `eye` toward `point`, in Minecraft's convention.
pub fn look_at(eye: [f64; 3], point: [f64; 3]) -> (f32, f32) {
    let [dx, dy, dz] = [point[0] - eye[0], point[1] - eye[1], point[2] - eye[2]];
    let yaw = (-dx).atan2(dz).to_degrees();
    let pitch = -dy.atan2(dx.hypot(dz)).to_degrees();
    (yaw as f32, pitch as f32)
}

/// Unit view vector for a camera orientation.
pub fn look_direction(yaw: f32, pitch: f32) -> [f64; 3] {
    let (yaw, pitch) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos()]
}

pub fn player_bounds(t: Transform) -> Aabb {
    Aabb {
        min: [t.x - HALF_WIDTH, t.y, t.z - HALF_WIDTH],
        max: [t.x + HALF_WIDTH, t.y + HEIGHT, t.z + HALF_WIDTH],
    }
}

/// Distance along the ray from `origin` in `direction` to where it enters
/// `bounds`, if it does (slab method).
pub fn ray_distance(origin: [f64; 3], direction: [f64; 3], bounds: Aabb) -> Option<f64> {
    let (mut near, mut far) = (0.0f64, f64::INFINITY);
    for axis in 0..3 {
        if direction[axis].abs() < 1e-9 {
            if origin[axis] < bounds.min[axis] || origin[axis] > bounds.max[axis] {
                return None;
            }
            continue;
        }
        let a = (bounds.min[axis] - origin[axis]) / direction[axis];
        let b = (bounds.max[axis] - origin[axis]) / direction[axis];
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// The crosshair meets the target within reach, with nothing solid between.
pub fn aimed_at(
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
    target: Transform,
    view: &impl BlockView,
) -> bool {
    let direction = look_direction(yaw, pitch);
    let Some(distance) = ray_distance(eye, direction, player_bounds(target)) else {
        return false;
    };
    let end = [0, 1, 2].map(|axis| eye[axis] + direction[axis] * distance);
    distance <= REACH && !blocked(view, eye, end)
}

/// Walking room between two players: no solid block at chest height along
/// the line, so glass walls separate boxes.
fn clear_path(view: &impl BlockView, from: Transform, to: Transform) -> bool {
    !blocked(view, [from.x, from.y + 1.0, from.z], [to.x, to.y + 1.0, to.z])
}

fn blocked(view: &impl BlockView, from: [f64; 3], to: [f64; 3]) -> bool {
    let length = (0..3).map(|axis| (to[axis] - from[axis]).powi(2)).sum::<f64>().sqrt();
    let steps = (length / 0.25).ceil().max(1.0) as u32;
    (0..=steps).any(|step| {
        let t = f64::from(step) / f64::from(steps);
        let [x, y, z] = [0, 1, 2].map(|axis| (from[axis] + (to[axis] - from[axis]) * t).floor());
        view.block_at(x as i32, y as i32, z as i32)
            .is_some_and(|state| !halcyon_world::state_info(state).collision_boxes().is_empty())
    })
}

#[cfg(test)]
#[path = "../tests/tests_bot_fighter.rs"]
mod tests;
