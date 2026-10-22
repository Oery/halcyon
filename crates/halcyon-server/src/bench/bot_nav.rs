//! Opt-in navigation scenario on a real map. No sockets or running server:
//! headless bots run the full brain, motor, navigation and physics stack.
//!
//! `HALCYON_TERRAIN_WORLD` selects the map (default `worlds/Parkour Spiral`),
//! `HALCYON_BENCH_BOTS` the bot count and `HALCYON_BENCH_TICKS` the duration.

use super::search::{Search, SearchStatus};
use super::*;
use crate::bot::brain::Mode;
use crate::bot::profile::BotProfile;
use crate::bot::sim::SimBot;
use crate::lobby::Transform;
use serde_json::{Value, json};

fn load_world() -> halcyon_world::World {
    let path = std::env::var_os("HALCYON_TERRAIN_WORLD").map_or_else(
        || {
            ["worlds/Parkour Spiral", "../../worlds/Parkour Spiral"]
                .into_iter()
                .map(std::path::PathBuf::from)
                .find(|path| path.exists())
                .expect("set HALCYON_TERRAIN_WORLD to a map directory")
        },
        std::path::PathBuf::from,
    );
    halcyon_world::World::load(&path).expect("map loads")
}

fn env_or(name: &str, default: u32) -> u32 {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}

#[test]
#[ignore = "opt-in release scenario: bots climb a real map headlessly"]
// An accidental debug run must fail loudly instead of recording misleading costs.
#[allow(clippy::assertions_on_constants)]
fn spire_climb_scenario() {
    assert!(!cfg!(debug_assertions), "run with --release");
    let world = load_world();
    let (x, y, z, yaw, pitch) = world.spawn;
    let spawn =
        crate::movement::resolve_spawn(&world, Transform { x, y, z, yaw, pitch, on_ground: false })
            .expect("spawn resolves");
    let mut nav = NavService::new(Budget::Unlimited);

    let started = std::time::Instant::now();
    let around = [spawn.x.floor() as i32, spawn.y.floor() as i32, spawn.z.floor() as i32];
    nav.goal = std::env::var("HALCYON_BOT_GOAL")
        .ok()
        .and_then(|goal| {
            let mut parts = goal.split(',').map(|part| part.trim().parse::<i32>().ok());
            Some([parts.next()??, parts.next()??, parts.next()??])
        })
        .or_else(|| nav.detect_goal(&world, around, 128));
    let goal = nav.goal.expect("a goal exists");
    eprintln!("spawn {spawn:?} goal {goal:?} detected in {}ms", started.elapsed().as_millis());

    // Route existence from spawn, independent of execution quality.
    let started = std::time::Instant::now();
    let mut search = Search::default();
    let start = super::stand::cell_of(spawn);
    search.start(start, goal);
    let route = loop {
        match search.step(&mut nav, &world, 100_000, &|_, _| 0.0) {
            SearchStatus::Running => {}
            done => break done,
        }
    };
    let elapsed = started.elapsed();
    let stats = nav.stats();
    let (kind, steps, end) = match &route {
        SearchStatus::Found(path) => ("found", path.len(), path.last().map(|s| s.edge.to)),
        SearchStatus::Partial(path) => ("partial", path.len(), path.last().map(|s| s.edge.to)),
        _ => ("unreachable", 0, None),
    };
    let per_sim = stats.build_time.as_secs_f64() * 1e6 / stats.simulations.max(1) as f64;
    eprintln!(
        "route: {kind} steps={steps} end={end:?} search_ms={} nodes={} edges={} expanded={} sims={} sim_ticks={} us_per_sim={per_sim:.1}",
        elapsed.as_millis(),
        stats.nodes,
        stats.edges,
        stats.expansions,
        stats.simulations,
        stats.simulated_ticks,
    );
    if let SearchStatus::Found(path) | SearchStatus::Partial(path) = &route {
        let jumps = path.iter().filter(|s| s.edge.kind == EdgeKind::Jump).count();
        let weakest = path.iter().map(|s| s.edge.robustness).fold(1.0f32, f32::min);
        eprintln!("route: jumps={jumps} weakest_robustness={weakest:.2}");
    }

    let count = env_or("HALCYON_BENCH_BOTS", 8);
    let limit = env_or("HALCYON_BENCH_TICKS", 12_000);
    let mut bots: Vec<SimBot> = (0..count)
        .map(|i| {
            let level = if count > 1 { (i * 100 / (count - 1)) as u8 } else { 50 };
            SimBot::new(BotProfile::new(level), u64::from(i) + 1, spawn)
        })
        .collect();
    let started = std::time::Instant::now();
    let mut slowest_tick = std::time::Duration::ZERO;
    for tick in 1..=limit {
        let tick_started = std::time::Instant::now();
        for bot in &mut bots {
            bot.step(&mut nav, &world);
        }
        slowest_tick = slowest_tick.max(tick_started.elapsed());
        if tick % 1_200 == 0 {
            let finished = bots.iter().filter(|b| b.brain.mode() == Mode::Finished).count();
            let best = bots.iter().map(|b| b.max_y).fold(f64::MIN, f64::max);
            eprintln!("t={}s finished={finished}/{count} best_y={best:.1}", tick / 20);
        }
        if bots.iter().all(|b| b.brain.mode() == Mode::Finished) {
            break;
        }
    }
    let mut rows = Vec::new();
    for bot in &bots {
        let stats = bot.brain.stats;
        eprintln!(
            "bot level={} mode={:?} max_y={:.1} y={:.1} finished_at={:?} falls={} deaths={} edges={}",
            bot.brain.profile.level,
            bot.brain.mode(),
            bot.max_y,
            bot.position().y,
            stats.finished_at,
            stats.falls,
            bot.deaths,
            stats.edges_done,
        );
        rows.push(json!({
            "level": bot.brain.profile.level,
            "max_y": bot.max_y,
            "finished_at": stats.finished_at,
            "falls": stats.falls,
            "deaths": bot.deaths,
        }));
    }
    let stats = nav.stats();
    eprintln!(
        "nav: wall_s={:.1} slowest_tick_ms={:.1} nodes={} edges={} sims={} build_ms={}",
        started.elapsed().as_secs_f64(),
        slowest_tick.as_secs_f64() * 1e3,
        stats.nodes,
        stats.edges,
        stats.simulations,
        stats.build_time.as_millis(),
    );
    let report: Value = json!({
        "goal": goal,
        "route": kind,
        "route_steps": steps,
        "us_per_sim": per_sim,
        "bots": rows,
    });
    if let Some(path) = std::env::var_os("HALCYON_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
