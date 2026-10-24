//! Headless bot duels under the real owner rules: two fighters in a `boxes`
//! box with the game's kit, driven exactly like live bots (publication, then
//! decisions that land on the next step), with simulated time. Reports how
//! strong each side is, which techniques it uses, and humanlike tells.
//!
//! `cargo test -p halcyon-server --release combat_duel_benchmark -- --ignored --nocapture`
//! `HALCYON_COMBAT_SCENARIO=<name>` runs one scenario, `HALCYON_COMBAT_DUELS=n`
//! sets duels per matchup (half with the entity IDs swapped) and
//! `HALCYON_BENCH_REPORT=<path>` writes JSON.

use std::rc::Rc;
use std::time::{Duration, Instant};

use mc_protocol::packets::Payload;
use mc_protocol::packets::client::PlayerInfoUpdate;
use serde_json::{Value, json};
use uuid::Uuid;

use super::registry::HitRecord;
use super::session::Input;
use super::{Profile, Registry, Session, Transform};
use crate::bot::fighter::{EYE, Fighter, Opponent, Tactics, look_direction, player_bounds};
use crate::bot::profile::BotProfile;
use crate::bot::{Body, act, perceive};
use crate::game::Destination;
use crate::game::boxes::{Kit, Layout, test_world};
use crate::inventory::HotbarSlot;
use crate::lifecycle::PlayerRules;

const STEP: Duration = Duration::from_millis(50);
/// A duel nobody wins within this is a draw.
const MAX_TICKS: u32 = 90 * 20;

/// One side of a matchup.
#[derive(Clone, Debug)]
pub(crate) struct Contender {
    pub label: String,
    pub level: u8,
    pub tactics: Option<Tactics>,
}

impl Contender {
    pub(crate) fn level(level: u8) -> Self {
        Self { label: format!("L{level}"), level, tactics: None }
    }

    fn with(label: &str, level: u8, tactics: Tactics) -> Self {
        Self { label: label.to_owned(), level, tactics: Some(tactics) }
    }

    fn fighter(&self, seed: u64, spawn: Transform) -> Fighter {
        let profile = BotProfile::new(self.level);
        match self.tactics {
            Some(tactics) => Fighter::with_tactics(profile, tactics, seed, spawn),
            None => Fighter::new(profile, seed, spawn),
        }
    }
}

/// Per-side measurements over one or many duels.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stats {
    pub ticks: u64,
    pub engaged_ticks: u64,
    pub clicks: u64,
    pub landed: u64,
    /// Hits the target's damage window swallowed or reduced to nothing.
    pub ignored: u64,
    pub sprint_hits: u64,
    pub crits: u64,
    pub damage_dealt: f64,
    pub strength_sum: f64,
    pub hit_distances: Vec<f64>,
    pub click_intervals: Vec<f64>,
    pub click_charges: Vec<f64>,
    pub aim_errors: Vec<f64>,
    pub turn_speeds: Vec<f64>,
    pub still_aim: u64,
    pub reversals: u64,
    pub wall_ticks: u64,
    pub idle_ticks: u64,
    pub combos: Vec<u32>,
    /// Fresh hits taken, and the horizontal distance each pushed this side
    /// over the next 8 ticks.
    pub knockback: Vec<f64>,
    pub jump_resets: u64,
    pub distance_sum: f64,
    pub counters: crate::bot::fighter::Counters,
}

impl Stats {
    fn merge(&mut self, other: &Stats) {
        self.ticks += other.ticks;
        self.engaged_ticks += other.engaged_ticks;
        self.clicks += other.clicks;
        self.landed += other.landed;
        self.ignored += other.ignored;
        self.sprint_hits += other.sprint_hits;
        self.crits += other.crits;
        self.damage_dealt += other.damage_dealt;
        self.strength_sum += other.strength_sum;
        self.hit_distances.extend(&other.hit_distances);
        self.click_intervals.extend(&other.click_intervals);
        self.click_charges.extend(&other.click_charges);
        self.aim_errors.extend(&other.aim_errors);
        self.turn_speeds.extend(&other.turn_speeds);
        self.still_aim += other.still_aim;
        self.reversals += other.reversals;
        self.wall_ticks += other.wall_ticks;
        self.idle_ticks += other.idle_ticks;
        self.combos.extend(&other.combos);
        self.knockback.extend(&other.knockback);
        self.jump_resets += other.jump_resets;
        self.distance_sum += other.distance_sum;
        let (a, b) = (&mut self.counters, other.counters);
        a.crit_plans += b.crit_plans;
        a.crit_swings += b.crit_swings;
        a.crit_landed += b.crit_landed;
        a.jump_resets += b.jump_resets;
        a.dodges += b.dodges;
        a.retreats += b.retreats;
        a.holds += b.holds;
        a.whiffs += b.whiffs;
    }
}

pub(crate) struct DuelResult {
    /// Index of the winning side, None on a draw.
    pub winner: Option<usize>,
    pub ticks: u32,
    pub stats: [Stats; 2],
}

/// Per-side duel bookkeeping the fighter does not expose.
#[derive(Default)]
struct Side {
    last_click: Option<u32>,
    last_look: Option<(f32, f32)>,
    strafe: i8,
    combo: u32,
    /// Fresh hits taken: tick, position and whether the jump key followed.
    pushes: Vec<(u32, Transform, bool)>,
}

/// One duel; `swap` gives the second contender the lower entity ID.
pub(crate) fn duel(
    world: &halcyon_world::World,
    layout: &Layout,
    contenders: [&Contender; 2],
    seed: u64,
    swap: bool,
    trace: bool,
) -> DuelResult {
    let mut registry = Registry::default();
    registry.arena.initialize(world).unwrap();
    for batch in layout.cells.chunks(halcyon_world::blocks::MAX_EDIT_CELLS) {
        registry.arena.edit_all(world, batch).unwrap();
    }
    let spawns = [layout.spawn(0, 0), layout.spawn(0, 1)];
    // HALCYON_COMBAT_KIT=long fights in the `boxes_long` kit.
    let kit = match std::env::var("HALCYON_COMBAT_KIT").as_deref() {
        Ok("long") => Kit::Long,
        _ => Kit::Iron,
    };
    let kit = kit.loadout().unwrap();
    let order = if swap { [1, 0] } else { [0, 1] };
    let mut sessions: [Option<Session>; 2] = [None, None];
    for side in order {
        let mut name = format!("{}{side}", contenders[side].label);
        name.retain(|c| c.is_ascii_alphanumeric() || c == '_');
        name.truncate(16);
        let uuid = Uuid::from_u128(seed as u128 * 2 + side as u128 + 1);
        let mut info = Vec::new();
        futures_lite::future::block_on(
            PlayerInfoUpdate { uuid, username: &name, properties: vec![] }.write_packet(&mut info),
        )
        .unwrap();
        let profile = Rc::new(Profile { uuid, username: name, info: Rc::from(info) });
        let input = Rc::new(Input::new(Rc::default()));
        let id = registry.reserve(profile, spawns[side], 0x7f, input.clone()).unwrap();
        let player = registry.players.get_mut(&id).unwrap();
        player.state.life.rules = PlayerRules::open(spawns[side]);
        player.state.inventory.replace_loadout(kit.clone(), HotbarSlot::FIRST).unwrap();
        player.state.equipment = Rc::new(player.state.inventory.equipment());
        let session = Session { entity_id: id, input };
        session.teleport(spawns[side]);
        sessions[side] = Some(session);
    }
    let sessions = sessions.map(Option::unwrap);
    let ids = [sessions[0].entity_id, sessions[1].entity_id];
    let mut fighters = [0, 1].map(|side| {
        let mut fighter =
            contenders[side].fighter(seed.wrapping_mul(31).wrapping_add(side as u64), spawns[side]);
        fighter.engaged = true;
        fighter
    });
    let mut bodies = [Body::default(), Body::default()];
    let mut hurt = [false; 2];
    let mut scratch = Vec::new();
    let mut stats = [Stats::default(), Stats::default()];
    let mut sides = [Side::default(), Side::default()];
    let start = Instant::now();
    let mut notices = Vec::new();
    let mut winner = None;
    let mut ticks = 0;
    let view_bounds = inner_bounds(layout);

    for tick in 0..MAX_TICKS {
        ticks = tick;
        let now = start + STEP * tick;
        registry.simulate(world, now, &mut notices);
        notices.clear();
        registry.publish_at(now);
        let hits: Vec<HitRecord> = std::mem::take(&mut registry.hits);
        let positions = ids.map(|id| registry.players[&id].state.movement.transform());
        for hit in &hits {
            let side = usize::from(hit.attacker == ids[1]);
            let other = 1 - side;
            let s = &mut stats[side];
            match hit.dealt {
                Some(dealt) if dealt > 0.0 => {
                    s.landed += 1;
                    s.damage_dealt += f64::from(dealt);
                    s.strength_sum += f64::from(hit.strength);
                    s.sprint_hits += u64::from(hit.sprint_knockback);
                    s.crits += u64::from(hit.crit);
                    let eye = [positions[side].x, positions[side].y + EYE, positions[side].z];
                    let bounds = player_bounds(positions[other]);
                    s.hit_distances.push(crate::combat::distance_sqr(bounds, eye).sqrt());
                    sides[side].combo += 1;
                    if sides[other].combo > 0 {
                        stats[other].combos.push(sides[other].combo);
                        sides[other].combo = 0;
                    }
                    sides[other].pushes.push((tick, positions[other], false));
                }
                _ => s.ignored += 1,
            }
            if trace {
                let movement = &registry.players[&ids[side]].state.movement;
                println!(
                    "  t={tick} hit {side}->{other} {hit:?} attacker ground={} sprinting={}",
                    movement.transform().on_ground,
                    movement.sprinting()
                );
            }
        }
        // Knockback taken: horizontal travel 8 ticks after each fresh hit.
        for side in 0..2 {
            let position = positions[side];
            sides[side].pushes.retain(|&(at, from, jumped)| {
                if tick - at < 8 {
                    return true;
                }
                stats[side].knockback.push((position.x - from.x).hypot(position.z - from.z));
                stats[side].jump_resets += u64::from(jumped);
                false
            });
        }
        for event in registry.events.drain(..) {
            if let crate::game::GameEvent::Died { player, .. } = event {
                winner = Some(usize::from(player == ids[0]));
            }
        }
        if winner.is_some() {
            break;
        }
        for side in 0..2 {
            let session = &sessions[side];
            let heard = bodies[side].sync(session);
            debug_assert!(
                heard.destination.is_none() || heard.destination == Some(Destination::Hub)
            );
            hurt[side] |= heard.hurt;
            if !session.ready() {
                continue;
            }
            let position = session.accepted_transform();
            let senses = perceive(&registry, ids[side], position, hurt[side], &mut scratch);
            let opponent: Option<Opponent> = senses.opponents.first().copied();
            let view = registry.arena.view(world);
            let decision = fighters[side].tick(&senses, &view);
            hurt[side] = false;
            act(session, &mut bodies[side], &decision);
            record(
                &mut stats[side],
                &mut sides[side],
                tick,
                position,
                opponent,
                senses.delay,
                &decision,
                fighters[side].engaged_target(),
                view_bounds,
            );
            if trace {
                let i = decision.input;
                println!(
                    "  t={tick} {side} pos=({:.2},{:.2},{:.2}) look=({:.1},{:.1}) keys={}{}{}{}{}{} attack={:?} {}",
                    position.x,
                    position.y,
                    position.z,
                    i.yaw,
                    i.pitch,
                    if i.forward { "W" } else { "-" },
                    if i.backward { "S" } else { "-" },
                    if i.left { "A" } else { "-" },
                    if i.right { "D" } else { "-" },
                    if i.jumping { "J" } else { "-" },
                    if i.sprinting { "R" } else { "-" },
                    decision.attack,
                    fighters[side].debug_state(),
                );
            }
        }
    }
    for side in 0..2 {
        if sides[side].combo > 0 {
            stats[side].combos.push(sides[side].combo);
        }
        stats[side].ticks = u64::from(ticks);
        stats[side].counters = fighters[side].counters;
    }
    DuelResult { winner, ticks, stats }
}

/// Inner floor bounds of the first box, for wall-contact measurement.
fn inner_bounds(layout: &Layout) -> [f64; 4] {
    let [bx, bz] = layout.boxes[0].map(f64::from);
    let inner = f64::from(crate::game::boxes::BOX_SIZE - 1);
    [bx + 1.0, bz + 1.0, bx + inner, bz + inner]
}

#[allow(clippy::too_many_arguments)]
fn record(
    stats: &mut Stats,
    side: &mut Side,
    tick: u32,
    me: Transform,
    opponent: Option<Opponent>,
    delay: f32,
    decision: &crate::bot::fighter::Decision,
    engaged: bool,
    [x0, z0, x1, z1]: [f64; 4],
) {
    let input = decision.input;
    if let Some((yaw, pitch)) = side.last_look {
        let turn = f64::from(crate::bot::motor::wrap_degrees(input.yaw - yaw))
            .hypot(f64::from(input.pitch - pitch));
        if engaged {
            stats.turn_speeds.push(turn * 20.0);
            stats.still_aim += u64::from(turn == 0.0);
        }
    }
    side.last_look = Some((input.yaw, input.pitch));
    let strafe = i8::from(input.right) - i8::from(input.left);
    if strafe != 0 {
        if side.strafe != 0 && strafe != side.strafe {
            stats.reversals += 1;
        }
        side.strafe = strafe;
    }
    if !engaged {
        return;
    }
    stats.engaged_ticks += 1;
    let margin = 0.45;
    if me.x < x0 + margin || me.x > x1 - margin || me.z < z0 + margin || me.z > z1 - margin {
        stats.wall_ticks += 1;
    }
    if !(input.forward || input.backward || input.left || input.right) {
        stats.idle_ticks += 1;
    }
    let Some(opponent) = opponent else { return };
    let t = opponent.transform;
    stats.distance_sum += (t.x - me.x).hypot(t.z - me.z);
    if let Some(last) = side.pushes.last_mut()
        && tick - last.0 <= 2
        && input.jumping
    {
        last.2 = true;
    }
    if decision.swing {
        stats.clicks += 1;
        let since = side.last_click.map_or(100, |last| tick - last);
        if since < 100 {
            stats.click_intervals.push(f64::from(since));
        }
        stats.click_charges.push(f64::from(((since as f32 + 0.5) / delay).min(1.0)));
        side.last_click = Some(tick);
        let eye = [me.x, me.y + EYE, me.z];
        let look = look_direction(input.yaw, input.pitch);
        let center = [t.x, t.y + 0.9, t.z];
        let to = [center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]];
        let length = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        let cos = (look[0] * to[0] + look[1] * to[1] + look[2] * to[2]) / length;
        stats.aim_errors.push(cos.clamp(-1.0, 1.0).acos().to_degrees());
    }
}

/// Aggregate of many duels between two contenders.
pub(crate) struct Matchup {
    pub label: String,
    pub duels: u32,
    pub wins: [u32; 2],
    pub draws: u32,
    pub ticks: u64,
    pub stats: [Stats; 2],
}

pub(crate) fn matchup(
    world: &halcyon_world::World,
    layout: &Layout,
    contenders: [&Contender; 2],
    duels: u32,
    seed: u64,
) -> Matchup {
    let mut result = Matchup {
        label: format!("{} vs {}", contenders[0].label, contenders[1].label),
        duels,
        wins: [0; 2],
        draws: 0,
        ticks: 0,
        stats: Default::default(),
    };
    for n in 0..duels {
        let duel = duel(world, layout, contenders, seed + u64::from(n), n % 2 == 1, false);
        match duel.winner {
            Some(side) => result.wins[side] += 1,
            None => result.draws += 1,
        }
        result.ticks += u64::from(duel.ticks);
        for side in 0..2 {
            result.stats[side].merge(&duel.stats[side]);
        }
    }
    result
}

/// Wilson 95% interval of a win share.
fn wilson(wins: f64, total: f64) -> (f64, f64) {
    if total == 0.0 {
        return (0.0, 1.0);
    }
    let z = 1.96;
    let p = wins / total;
    let denominator = 1.0 + z * z / total;
    let center = (p + z * z / (2.0 * total)) / denominator;
    let spread = z * (p * (1.0 - p) / total + z * z / (4.0 * total * total)).sqrt() / denominator;
    (center - spread, center + spread)
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn deviation(values: &[f64]) -> f64 {
    let m = mean(values);
    (values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len().max(1) as f64).sqrt()
}

/// Humanlike tells for one side: (name, measured, threshold) breaches.
pub(crate) fn tells(stats: &Stats) -> Vec<String> {
    let mut tells = Vec::new();
    let engaged = stats.engaged_ticks.max(1) as f64;
    let max_turn = percentile(&stats.turn_speeds, 1.0);
    if max_turn > 2400.0 {
        tells.push(format!("camera snap: max turn {max_turn:.0} deg/s > 2400"));
    }
    let still = stats.still_aim as f64 / engaged;
    if still > 0.2 {
        tells.push(format!("frozen aim: {:.0}% of ticks without rotation > 20%", still * 100.0));
    }
    if stats.click_intervals.len() > 20 {
        let spread = deviation(&stats.click_intervals);
        if spread < 0.5 {
            tells.push(format!("metronome clicks: interval sd {spread:.2} ticks < 0.5"));
        }
    }
    let reversals = stats.reversals as f64 / (stats.ticks.max(1) as f64 / 20.0);
    if reversals > 3.0 {
        tells.push(format!("jitter: {reversals:.1} strafe reversals/s > 3"));
    }
    let wall = stats.wall_ticks as f64 / engaged;
    if wall > 0.35 {
        tells.push(format!("wall hugging: {:.0}% of engaged ticks > 35%", wall * 100.0));
    }
    let idle = stats.idle_ticks as f64 / engaged;
    if idle > 0.25 {
        tells.push(format!("standing still: {:.0}% of engaged ticks > 25%", idle * 100.0));
    }
    if stats.clicks > 20 {
        let perfect = mean(&stats.aim_errors);
        if perfect < 0.5 {
            tells.push(format!("pixel aim: mean aim error {perfect:.2} deg < 0.5"));
        }
    }
    if stats.clicks > 50 {
        let accuracy = stats.landed as f64 / stats.clicks as f64;
        if accuracy > 0.98 {
            tells.push(format!("never misses: {:.1}% of swings land > 98%", accuracy * 100.0));
        }
    }
    tells
}

fn side_json(stats: &Stats) -> Value {
    let seconds = stats.ticks.max(1) as f64 / 20.0;
    json!({
        "clicks_per_s": stats.clicks as f64 / seconds,
        "accuracy": stats.landed as f64 / stats.clicks.max(1) as f64,
        "ignored": stats.ignored,
        "damage_per_duel_s": stats.damage_dealt / seconds,
        "mean_strength": stats.strength_sum / stats.landed.max(1) as f64,
        "sprint_hit_share": stats.sprint_hits as f64 / stats.landed.max(1) as f64,
        "crit_share": stats.crits as f64 / stats.landed.max(1) as f64,
        "hit_distance_p50": percentile(&stats.hit_distances, 0.5),
        "hit_distance_p90": percentile(&stats.hit_distances, 0.9),
        "click_charge_mean": mean(&stats.click_charges),
        "click_interval_sd": deviation(&stats.click_intervals),
        "aim_error_mean": mean(&stats.aim_errors),
        "turn_p50": percentile(&stats.turn_speeds, 0.5),
        "turn_p95": percentile(&stats.turn_speeds, 0.95),
        "turn_max": percentile(&stats.turn_speeds, 1.0),
        "combo_mean": mean(&stats.combos.iter().map(|&c| f64::from(c)).collect::<Vec<_>>()),
        "combo_max": stats.combos.iter().max().copied().unwrap_or(0),
        "knockback_taken_mean": mean(&stats.knockback),
        "jump_reset_share": stats.jump_resets as f64 / stats.knockback.len().max(1) as f64,
        "mean_distance": stats.distance_sum / stats.engaged_ticks.max(1) as f64,
        "wall_share": stats.wall_ticks as f64 / stats.engaged_ticks.max(1) as f64,
        "counters": format!("{:?}", stats.counters),
        "tells": tells(stats),
    })
}

fn print_matchup(result: &Matchup) -> Value {
    let decided = f64::from(result.wins[0] + result.wins[1]);
    let share = f64::from(result.wins[0]) / decided.max(1.0);
    let (low, high) = wilson(f64::from(result.wins[0]), decided);
    let seconds = result.ticks as f64 / f64::from(result.duels.max(1)) / 20.0;
    println!(
        "{:<36} wins {:>4}-{:<4} draws {:>3}  first {:>5.1}% [{:>4.1}, {:>4.1}]  duel {:>5.1}s",
        result.label,
        result.wins[0],
        result.wins[1],
        result.draws,
        share * 100.0,
        low * 100.0,
        high * 100.0,
        seconds,
    );
    let mut sides = Vec::new();
    for (index, stats) in result.stats.iter().enumerate() {
        let side = side_json(stats);
        println!(
            "    side {index}: cps {:.2} acc {:.0}% str {:.2} sprint {:.0}% crit {:.0}% dist p50 {:.2} combo {:.2}/{} kb {:.2} jr {:.0}% gap {:.2} wall {:.0}% aim {:.1}deg turn p95 {:.0} max {:.0} isd {:.2} ignored {}",
            side["clicks_per_s"].as_f64().unwrap(),
            side["accuracy"].as_f64().unwrap() * 100.0,
            side["mean_strength"].as_f64().unwrap(),
            side["sprint_hit_share"].as_f64().unwrap() * 100.0,
            side["crit_share"].as_f64().unwrap() * 100.0,
            side["hit_distance_p50"].as_f64().unwrap(),
            side["combo_mean"].as_f64().unwrap(),
            side["combo_max"],
            side["knockback_taken_mean"].as_f64().unwrap(),
            side["jump_reset_share"].as_f64().unwrap() * 100.0,
            side["mean_distance"].as_f64().unwrap(),
            side["wall_share"].as_f64().unwrap() * 100.0,
            side["aim_error_mean"].as_f64().unwrap(),
            side["turn_p95"].as_f64().unwrap(),
            side["turn_max"].as_f64().unwrap(),
            side["click_interval_sd"].as_f64().unwrap(),
            side["ignored"],
        );
        if std::env::var_os("HALCYON_COMBAT_COUNTERS").is_some() {
            println!("      {:?}", stats.counters);
        }
        for tell in tells(stats) {
            println!("    TELL side {index}: {tell}");
        }
        sides.push(side);
    }
    json!({
        "label": result.label,
        "duels": result.duels,
        "wins": result.wins,
        "draws": result.draws,
        "first_share": share,
        "first_share_low": low,
        "first_share_high": high,
        "duel_seconds": seconds,
        "sides": sides,
    })
}

fn scenarios(full: Tactics) -> Vec<(&'static str, Vec<[Contender; 2]>)> {
    let ladder = [0u8, 25, 50, 75, 100];
    let mut round_robin = Vec::new();
    for (i, &a) in ladder.iter().enumerate() {
        for &b in &ladder[i + 1..] {
            round_robin.push([Contender::level(b), Contender::level(a)]);
        }
    }
    let ablations = Tactics::ablations(full)
        .into_iter()
        .map(|(name, tactics)| {
            [Contender::with("full", 75, full), Contender::with(name, 75, tactics)]
        })
        .collect();
    vec![
        ("baseline", vec![[Contender::level(75), Contender::with("basic", 75, Tactics::basic())]]),
        (
            "mirror",
            vec![
                [Contender::level(50), Contender::level(50)],
                [Contender::level(90), Contender::level(90)],
            ],
        ),
        ("bias", vec![[Contender::with("a", 75, full), Contender::with("b", 75, full)]]),
        ("ladder", round_robin),
        ("ablation", ablations),
        ("probe", {
            // One level-100 knob set to its level-50 value at a time.
            let top = Tactics::scaled(full, BotProfile::new(100), 0.5, 0.5);
            let mid = Tactics::scaled(full, BotProfile::new(50), 0.5, 0.5);
            [
                ("timing", Tactics { timing_sd: mid.timing_sd, ..top }),
                ("lead", Tactics { lead: mid.lead, ..top }),
                ("pursuit", Tactics { pursuit: mid.pursuit, ..top }),
                ("iframes", Tactics { respect_iframes: mid.respect_iframes, ..top }),
                ("crits", Tactics { crits: mid.crits, ..top }),
                ("allcrits", Tactics { crits: 1.0, ..top }),
                ("jumpreset", Tactics { jump_reset: mid.jump_reset, ..top }),
                ("evasion", Tactics { evasion: mid.evasion, ..top }),
            ]
            .into_iter()
            .map(|(name, tactics)| {
                [Contender::with(name, 100, tactics), Contender::with("top", 100, top)]
            })
            .collect()
        }),
        ("isolate", {
            // Strong hands with middling tactics, and middling hands with
            // strong tactics, each against a plain level-50 bot.
            let tactics = |level| Tactics::scaled(full, BotProfile::new(level), 0.5, 0.5);
            let mut pairs = Vec::new();
            for level in [60, 70, 80, 90, 100] {
                pairs.push([
                    Contender::with(&format!("hands{level}"), level, tactics(50)),
                    Contender::with("plain50", 50, tactics(50)),
                ]);
                pairs.push([
                    Contender::with(&format!("tactics{level}"), 50, tactics(level)),
                    Contender::with("plain50", 50, tactics(50)),
                ]);
            }
            pairs
        }),
    ]
}

#[test]
#[ignore = "bot combat benchmark; run explicitly in release"]
fn combat_duel_benchmark() {
    let world = test_world();
    let layout = Layout::new(&world).unwrap();
    let duels: u32 =
        std::env::var("HALCYON_COMBAT_DUELS").ok().and_then(|n| n.parse().ok()).unwrap_or(200);
    let only = std::env::var("HALCYON_COMBAT_SCENARIO").ok();
    let full = Tactics::for_level(BotProfile::new(75), 0.5, 0.5);
    let started = Instant::now();
    let mut report = Vec::new();
    for (name, matchups) in scenarios(full) {
        if only.as_deref().is_some_and(|only| only != name) {
            continue;
        }
        println!("== {name} ({duels} duels per matchup)");
        for (index, [a, b]) in matchups.iter().enumerate() {
            let result = matchup(&world, &layout, [a, b], duels, 1_000 * index as u64 + 17);
            report.push(json!({ "scenario": name, "result": print_matchup(&result) }));
        }
    }
    println!("finished in {:.1}s", started.elapsed().as_secs_f64());
    if let Ok(path) = std::env::var("HALCYON_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
}

/// One duel, tick by tick: `HALCYON_DUEL=<levelA>,<levelB>,<seed>`.
#[test]
#[ignore = "combat forensics; run explicitly"]
fn trace_duel() {
    let world = test_world();
    let layout = Layout::new(&world).unwrap();
    let spec = std::env::var("HALCYON_DUEL").unwrap_or_else(|_| "75,75,1".to_owned());
    let parts: Vec<u64> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    let (a, b, seed) = (parts[0] as u8, parts[1] as u8, parts[2]);
    let basic = std::env::var_os("HALCYON_DUEL_BASIC").is_some();
    // HALCYON_DUEL_CRITS=p forces the critical-jump chance, to study crits.
    let crits = std::env::var("HALCYON_DUEL_CRITS").ok().and_then(|p| p.parse().ok());
    let make = |level| {
        if basic {
            Contender::with("basic", level, Tactics::basic())
        } else if let Some(tactics) =
            std::env::var("HALCYON_DUEL_TACTICS").ok().and_then(|l| l.parse().ok())
        {
            // Tactics of this level on the given level's hands.
            Contender::with("t", level, Tactics::for_level(BotProfile::new(tactics), 0.5, 0.5))
        } else if let Some(crits) = crits {
            let tactics = Tactics::for_level(BotProfile::new(level), 0.5, 0.5);
            Contender::with("crit", level, Tactics { crits, ..tactics })
        } else {
            Contender::level(level)
        }
    };
    let (a, b) = (make(a), make(b));
    let result = duel(&world, &layout, [&a, &b], seed, false, true);
    println!("winner {:?} after {} ticks", result.winner, result.ticks);
    for (side, stats) in result.stats.iter().enumerate() {
        println!("side {side}: {}", side_json(stats));
    }
}

/// The `ladder` game under the real owner rules for three simulated minutes,
/// with a fixed-level bot standing in for the player (`HALCYON_LADDER_PLAYER`,
/// default 70). Prints the game's messages to the player.
#[test]
#[ignore = "ladder game run; run explicitly in release"]
fn ladder_run() {
    use super::{GameCtx, InstanceId};
    use crate::game::{Game, GameEvent, Ladder};
    let world = test_world();
    let player_level: u8 =
        std::env::var("HALCYON_LADDER_PLAYER").ok().and_then(|l| l.parse().ok()).unwrap_or(70);
    let mut game = Ladder::new(&world, "Rival1".to_owned()).unwrap();
    let sparring = game.sparring().unwrap();
    let mut registry = Registry::default();
    registry.arena.initialize(&world).unwrap();
    let (mut notices, mut close) = (Vec::new(), false);
    let id = InstanceId { shard: 0, local: 0 };
    macro_rules! ctx {
        () => {
            GameCtx {
                id,
                registry: &mut registry,
                world: &world,
                notices: &mut notices,
                close: &mut close,
            }
        };
    }
    game.on_create(&mut ctx!());
    let mut sessions = Vec::new();
    for (index, name) in ["Rival1", "Player"].into_iter().enumerate() {
        let uuid = Uuid::from_u128(index as u128 + 1);
        let mut info = Vec::new();
        futures_lite::future::block_on(
            PlayerInfoUpdate { uuid, username: name, properties: vec![] }.write_packet(&mut info),
        )
        .unwrap();
        let profile = Rc::new(Profile { uuid, username: name.to_owned(), info: Rc::from(info) });
        let input = Rc::new(Input::new(Rc::default()));
        let (x, y, z, yaw, pitch) = world.spawn;
        let spawn = Transform { x, y, z, yaw, pitch, on_ground: false };
        let entity = registry.reserve(profile, spawn, 0x7f, input.clone()).unwrap();
        game.on_join(&mut ctx!(), entity);
        let session = Session { entity_id: entity, input };
        session.teleport(session.accepted_transform());
        sessions.push(session);
    }
    let ids = [sessions[0].entity_id, sessions[1].entity_id];
    let fighter =
        |level: u8, seed: u64| Fighter::new(BotProfile::new(level), seed, Transform::default());
    let mut fighters = [fighter(sparring.level.get(), 1), fighter(player_level, 2)];
    let mut generation = sparring.generation.get();
    let mut bodies = [Body::default(), Body::default()];
    let (mut hurt, mut scratch) = ([false; 2], Vec::new());
    let start = Instant::now();
    for tick in 0..20 * 180 {
        let now = start + STEP * tick;
        registry.simulate(&world, now, &mut notices);
        let events: Vec<GameEvent> = std::mem::take(&mut registry.events);
        for event in events {
            game.on_event(&mut ctx!(), event);
        }
        game.tick(&mut ctx!());
        notices.clear();
        registry.publish_at(now);
        registry.hits.clear();
        if sparring.generation.get() != generation {
            generation = sparring.generation.get();
            fighters[0] = fighter(sparring.level.get(), u64::from(generation) + 10);
        }
        for side in 0..2 {
            let session = &sessions[side];
            if side == 1 {
                for line in &session.input.update.borrow().messages {
                    println!("{:>5.1}s  {line}", f64::from(tick) / 20.0);
                }
            }
            let heard = bodies[side].sync(session);
            hurt[side] |= heard.hurt;
            if heard.died {
                fighters[side].engaged = false;
            }
            if heard.fight {
                fighters[side].engaged = true;
            }
            if !session.ready() {
                continue;
            }
            let position = session.accepted_transform();
            let senses = perceive(&registry, ids[side], position, hurt[side], &mut scratch);
            let decision = fighters[side].tick(&senses, &registry.arena.view(&world));
            hurt[side] = false;
            act(session, &mut bodies[side], &decision);
        }
    }
}

/// One tunable knob of `MASTER`: a name and a step in either direction,
/// None when the step leaves its range.
type Mutator = (&'static str, fn(Tactics, i32) -> Option<Tactics>);

fn step_f64(value: f64, by: f64, dir: i32, (low, high): (f64, f64)) -> Option<f64> {
    let next = value + by * f64::from(dir);
    (low - 1e-9..=high + 1e-9).contains(&next).then_some(next)
}

fn step_f32(value: f32, by: f32, dir: i32, range: (f32, f32)) -> Option<f32> {
    step_f64(f64::from(value), f64::from(by), dir, (f64::from(range.0), f64::from(range.1)))
        .map(|next| next as f32)
}

fn step_int(value: i32, by: i32, dir: i32, (low, high): (i32, i32)) -> Option<i32> {
    let next = value + by * dir;
    (low..=high).contains(&next).then_some(next)
}

fn mutators() -> Vec<Mutator> {
    use crate::bot::fighter::SprintReset;
    vec![
        ("timing_sd", |t, d| {
            Some(Tactics { timing_sd: step_f32(t.timing_sd, 0.25, d, (0.0, 2.5))?, ..t })
        }),
        ("respect_iframes", |t, d| {
            Some(Tactics {
                respect_iframes: step_f32(t.respect_iframes, 0.25, d, (0.0, 1.0))?,
                ..t
            })
        }),
        ("spacing", |t, d| (d > 0).then_some(Tactics { spacing: !t.spacing, ..t })),
        ("hover", |t, d| Some(Tactics { hover: step_f64(t.hover, 0.2, d, (2.8, 5.0))?, ..t })),
        ("wait_gap", |t, d| {
            Some(Tactics { wait_gap: step_f64(t.wait_gap, 0.1, d, (2.6, 4.0))?, ..t })
        }),
        ("arrive_slack", |t, d| {
            Some(Tactics { arrive_slack: step_f64(t.arrive_slack, 1.0, d, (-1.0, 4.0))?, ..t })
        }),
        ("sprint_reset", |t, d| {
            let modes =
                [SprintReset::Hold, SprintReset::WTap, SprintReset::STap, SprintReset::Adaptive];
            let index = modes.iter().position(|m| *m == t.sprint_reset)? as i32;
            Some(Tactics { sprint_reset: modes[(index + d).rem_euclid(4) as usize], ..t })
        }),
        ("tap_ticks", |t, d| {
            Some(Tactics { tap_ticks: step_int(t.tap_ticks as i32, 1, d, (1, 6))? as u32, ..t })
        }),
        ("stap_gap", |t, d| {
            Some(Tactics { stap_gap: step_f64(t.stap_gap, 0.3, d, (0.0, 3.0))?, ..t })
        }),
        ("crits", |t, d| Some(Tactics { crits: step_f32(t.crits, 0.1, d, (0.0, 1.0))?, ..t })),
        ("crit_lead", |t, d| {
            Some(Tactics { crit_lead: step_int(t.crit_lead, 1, d, (2, 10))?, ..t })
        }),
        ("jump_reset", |t, d| {
            Some(Tactics { jump_reset: step_f32(t.jump_reset, 0.15, d, (0.0, 1.0))?, ..t })
        }),
        ("hit_select", |t, d| {
            Some(Tactics { hit_select: step_f32(t.hit_select, 0.2, d, (0.0, 1.0))?, ..t })
        }),
        ("combo", |t, d| (d > 0).then_some(Tactics { combo: !t.combo, ..t })),
        ("chase_speed", |t, d| {
            Some(Tactics { chase_speed: step_f64(t.chase_speed, 0.04, d, (0.02, 0.4))?, ..t })
        }),
        ("evasion", |t, d| {
            Some(Tactics { evasion: step_f32(t.evasion, 0.2, d, (0.0, 1.0))?, ..t })
        }),
        ("retreat", |t, d| (d > 0).then_some(Tactics { retreat: !t.retreat, ..t })),
        ("retreat_health", |t, d| {
            Some(Tactics { retreat_health: step_f32(t.retreat_health, 2.0, d, (0.0, 14.0))?, ..t })
        }),
        ("retreat_streak", |t, d| {
            Some(Tactics {
                retreat_streak: step_int(t.retreat_streak as i32, 1, d, (1, 4))? as u32,
                ..t
            })
        }),
        ("strafe_min", |t, d| {
            let min = step_int(t.strafe_min as i32, 3, d, (2, 20))? as u32;
            (min + 2 <= t.strafe_max).then_some(Tactics { strafe_min: min, ..t })
        }),
        ("strafe_max", |t, d| {
            let max = step_int(t.strafe_max as i32, 8, d, (6, 60))? as u32;
            (t.strafe_min + 2 <= max).then_some(Tactics { strafe_max: max, ..t })
        }),
        ("strafe_still", |t, d| {
            Some(Tactics { strafe_still: step_f32(t.strafe_still, 0.1, d, (0.0, 0.6))?, ..t })
        }),
    ]
}

/// A master's level-100 contender, with middling traits.
fn master_contender(label: &str, master: Tactics) -> Contender {
    Contender::with(label, 100, Tactics::scaled(master, BotProfile::new(100), 0.5, 0.5))
}

/// Wins and decided duels of `candidate` against each of `pool`, with the
/// same seeds for every candidate, plus the candidate's tells.
fn evaluate(
    world: &halcyon_world::World,
    layout: &Layout,
    candidate: &Contender,
    pool: &[Contender],
    duels: u32,
) -> (u32, u32, Vec<String>) {
    let (mut wins, mut decided) = (0, 0);
    let mut stats = Stats::default();
    for (index, opponent) in pool.iter().enumerate() {
        let seed = 50_000 + 1_000 * index as u64;
        let result = matchup(world, layout, [candidate, opponent], duels, seed);
        wins += result.wins[0];
        decided += result.wins[0] + result.wins[1];
        stats.merge(&result.stats[0]);
    }
    (wins, decided, tells(&stats))
}

/// Coordinate search over `MASTER`'s knobs: a step is kept when it beats the
/// current best and recent bests significantly, without tells. Prints the
/// result to paste into `MASTER`. `HALCYON_TUNE_DUELS` (per pairing, default
/// 300) and `HALCYON_TUNE_ROUNDS` (default 3).
#[test]
#[ignore = "combat tactics tuner; run explicitly in release"]
fn tune_tactics() {
    use crate::bot::fighter::MASTER;
    let world = test_world();
    let layout = Layout::new(&world).unwrap();
    let env = |name: &str, default: u32| {
        std::env::var(name).ok().and_then(|n| n.parse().ok()).unwrap_or(default)
    };
    let (duels, rounds) = (env("HALCYON_TUNE_DUELS", 300), env("HALCYON_TUNE_ROUNDS", 3));
    let mut best = MASTER;
    let mut archive = vec![MASTER];
    let started = Instant::now();
    for round in 0..rounds {
        let mut improved = false;
        for (name, mutate) in mutators() {
            for dir in [-1, 1] {
                let Some(candidate) = mutate(best, dir) else { continue };
                let mut pool = vec![master_contender("best", best)];
                pool.extend(archive.iter().rev().take(3).map(|m| master_contender("old", *m)));
                let candidate_side = master_contender("cand", candidate);
                let (wins, decided, tells) =
                    evaluate(&world, &layout, &candidate_side, &pool, duels);
                let share = f64::from(wins) / f64::from(decided.max(1));
                let (low, _) = wilson(f64::from(wins), f64::from(decided));
                let accepted = low > 0.5 && tells.is_empty();
                println!(
                    "round {round} {name:>16} {dir:+}: {:.1}% [{:.1}] {}{}",
                    share * 100.0,
                    low * 100.0,
                    if accepted { "ACCEPT" } else { "" },
                    if tells.is_empty() {
                        String::new()
                    } else {
                        format!(" tells {tells:?}")
                    },
                );
                if accepted {
                    archive.push(best);
                    best = candidate;
                    improved = true;
                    break;
                }
            }
        }
        if !improved {
            println!("round {round}: no improvement, stopping");
            break;
        }
    }
    println!("tuned in {:.0}s:\n{best:#?}", started.elapsed().as_secs_f64());
}
