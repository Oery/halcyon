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

/// The terrain overlay as the live lobby has it once loose blocks settle:
/// floating sand and gravel fall at startup.
fn settled(world: &halcyon_world::World) -> crate::arena::Arena {
    let mut arena = crate::arena::Arena::default();
    arena.initialize(world).expect("arena");
    let mut id = 0;
    for _ in 0..400 {
        arena.step(world, &mut id);
    }
    arena
}

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

fn block_names() -> std::collections::HashMap<u32, &'static str> {
    halcyon_world::blockstates::BLOCK_STATES.iter().map(|&(key, id)| (id, key)).collect()
}

/// Map survey: what surrounds spawn and how high the terrain reaches.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_map() {
    let world = load_world();
    let names = block_names();
    let name = |id: u16| names.get(&u32::from(id)).copied().unwrap_or("?");
    let (sx, sy, sz, _, _) = world.spawn;
    let [sx, sy, sz] = [sx.floor() as i32, sy.floor() as i32, sz.floor() as i32];
    eprintln!("spawn column at ({sx},{sz}):");
    for y in (sy - 4..=sy + 3).rev() {
        eprintln!("  y={y} {}", world.block_at(sx, y, sz).map_or("-", name));
    }
    // Top surface heights in a square around a centre, as a coarse ASCII map.
    let centre = std::env::var("HALCYON_SURVEY_AT")
        .ok()
        .and_then(|at| {
            let mut parts = at.split(',').map(|p| p.trim().parse::<i32>().ok());
            Some([parts.next()??, parts.next()??])
        })
        .unwrap_or([sx, sz]);
    let radius = env_or("HALCYON_SURVEY_RADIUS", 40) as i32;
    let mut counts: std::collections::HashMap<&str, usize> = Default::default();
    eprintln!(
        "top surface around {centre:?} (digit = y % 10, '~' water, 'o' lily pad, '*' leaves):"
    );
    let step = env_or("HALCYON_SURVEY_STEP", 2) as usize;
    for z in (centre[1] - radius..=centre[1] + radius).step_by(step) {
        let mut row = String::new();
        for x in (centre[0] - radius..=centre[0] + radius).step_by(step) {
            let top = (halcyon_world::MIN_BLOCK_Y..halcyon_world::MAX_BLOCK_Y)
                .rev()
                .find_map(|y| world.block_at(x, y, z).filter(|&id| id != 0).map(|id| (y, id)));
            row.push(match top {
                Some((y, id)) => {
                    let block = name(id).split('[').next().unwrap();
                    *counts.entry(block).or_default() += 1;
                    match block {
                        "minecraft:water" => '~',
                        "minecraft:lily_pad" => 'o',
                        b if b.ends_with("leaves") => '*',
                        _ => char::from(b'0' + y.rem_euclid(10) as u8),
                    }
                }
                None => '.',
            });
        }
        eprintln!("  {row}");
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|a| std::cmp::Reverse(a.1));
    eprintln!("top-surface blocks: {:?}", &counts[..counts.len().min(25)]);
}

/// Horizontal slices of the terrain, one character per block.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_layers() {
    let world = load_world();
    let arena = settled(&world);
    let view = arena.view(&world);
    let names = block_names();
    let at = std::env::var("HALCYON_SURVEY_AT").unwrap_or_else(|_| "0,54,-45".into());
    let mut parts = at.split(',').map(|p| p.trim().parse::<i32>().unwrap());
    let [cx, cy, cz] = [parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap()];
    let radius = env_or("HALCYON_SURVEY_RADIUS", 12) as i32;
    let layers = env_or("HALCYON_SURVEY_LAYERS", 8) as i32;
    let mut legend: std::collections::BTreeMap<char, std::collections::BTreeSet<&str>> =
        Default::default();
    for y in (cy - 2..cy - 2 + layers).rev() {
        eprintln!(
            "y={y} (x {}..{} left to right, z {}..{} top to bottom)",
            cx - radius,
            cx + radius,
            cz - radius,
            cz + radius
        );
        for z in cz - radius..=cz + radius {
            let mut row = String::new();
            for x in cx - radius..=cx + radius {
                let id = view.block_at(x, y, z).unwrap_or(0);
                let name = names.get(&u32::from(id)).copied().unwrap_or("?");
                let base = name.split('[').next().unwrap().trim_start_matches("minecraft:");
                let boxes = halcyon_world::state_info(id).collision_boxes();
                let c = if id == 0 {
                    '.'
                } else if base == "water" {
                    '~'
                } else if base.ends_with("leaves") {
                    '*'
                } else if base == "ladder" {
                    'H'
                } else if base == "vine" {
                    'v'
                } else if boxes.is_empty() {
                    ','
                } else if boxes.iter().any(|b| b.max[1] >= 1.0 && b.min[1] <= 0.0)
                    && boxes.len() == 1
                {
                    '#'
                } else if base.ends_with("slab") {
                    's'
                } else if base.ends_with("stairs") {
                    'z'
                } else if base.ends_with("fence") || base.ends_with("wall") {
                    'f'
                } else {
                    '?'
                };
                if !matches!(c, '.' | '~' | '#') {
                    legend.entry(c).or_default().insert(base);
                }
                if [x, y, z] == [cx, cy, cz] {
                    row.push('@');
                } else {
                    row.push(c);
                }
            }
            eprintln!("  {row}");
        }
    }
    for (c, kinds) in legend {
        eprintln!("{c}: {:?}", kinds.into_iter().take(12).collect::<Vec<_>>());
    }
}

/// Every non-air block in a small box, by name.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_box() {
    let world = load_world();
    let names = block_names();
    let parse = |name: &str, default: &str| {
        let text = std::env::var(name).unwrap_or_else(|_| default.into());
        let parts: Vec<i32> = text.split(',').map(|p| p.trim().parse().unwrap()).collect();
        [parts[0], parts[1], parts[2]]
    };
    let min = parse("HALCYON_BOX_MIN", "2,52,-47");
    let max = parse("HALCYON_BOX_MAX", "7,60,-41");
    let mut nav = NavService::new(Budget::Unlimited);
    // HALCYON_BOX_LIVE: the terrain as the live lobby has it once settled.
    let arena = if std::env::var("HALCYON_BOX_LIVE").is_ok() {
        settled(&world)
    } else {
        crate::arena::Arena::default()
    };
    let view = arena.view(&world);
    for y in (min[1]..=max[1]).rev() {
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                let id = view.block_at(x, y, z).unwrap_or(0);
                let stand = nav.stand(&view, [x, y, z]).map(|s| s.feet_y);
                if id != 0 || stand.is_some() {
                    let name = names[&u32::from(id)].trim_start_matches("minecraft:");
                    eprintln!("  [{x},{y},{z}] {name} stand={stand:?}");
                }
            }
        }
    }
}

/// Water above lake level near the spire base: waterfalls and elevators.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_raised_water() {
    let world = load_world();
    let mut columns: std::collections::BTreeMap<(i32, i32), (i32, i32, usize)> = Default::default();
    for x in -60..=40 {
        for z in -70..=10 {
            for y in 53..=130 {
                let Some(id) = world.block_at(x, y, z) else { continue };
                let fluid = halcyon_world::fluid_info(id);
                if fluid.kind == halcyon_world::FluidKind::Water {
                    let entry = columns.entry((x, z)).or_insert((y, y, 0));
                    entry.0 = entry.0.min(y);
                    entry.1 = entry.1.max(y);
                    entry.2 += 1;
                }
            }
        }
    }
    // Tall columns are waterfalls or elevators; report the lowest ones first.
    let mut tall: Vec<_> =
        columns.into_iter().filter(|(_, (low, high, _))| high - low >= 3).collect();
    tall.sort_by_key(|(_, (low, _, _))| *low);
    for ((x, z), (low, high, count)) in tall.iter().take(30) {
        let bubble = (*low..=*high).any(|y| {
            world.block_at(*x, y, *z).is_some_and(|id| {
                halcyon_world::blockstates::BLOCK_STATES
                    .iter()
                    .any(|(key, state)| *state == u32::from(id) && key.contains("bubble_column"))
            })
        });
        eprintln!("  column ({x},{z}) water y {low}..={high} ({count} blocks) bubbles={bubble}");
    }
}

/// Gold pressure plates (checkpoints): where they are, sorted by height.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_checkpoints() {
    let world = load_world();
    let plates: std::collections::HashSet<u16> = halcyon_world::blockstates::BLOCK_STATES
        .iter()
        .filter(|(key, _)| key.starts_with("minecraft:light_weighted_pressure_plate"))
        .map(|&(_, id)| id as u16)
        .collect();
    let mut nav = NavService::new(Budget::Unlimited);
    let mut found = Vec::new();
    for x in -128..=128 {
        for z in -228..=28 {
            for y in halcyon_world::MIN_BLOCK_Y..halcyon_world::MAX_BLOCK_Y {
                if world.block_at(x, y, z).is_some_and(|id| plates.contains(&id)) {
                    found.push([x, y, z]);
                }
            }
        }
    }
    found.sort_by_key(|c| c[1]);
    for cell in &found {
        eprintln!("  plate {cell:?} stand={:?}", nav.stand(&world, *cell).map(|s| s.feet_y));
    }
    eprintln!("{} gold plates", found.len());
}

/// Floors from which a cell could be jumped to: within reach, at most a
/// jump's height below and a few blocks above. Shows how a goal is entered.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_approaches() {
    let world = load_world();
    let names = block_names();
    let text = std::env::var("HALCYON_SURVEY_AT").unwrap_or_else(|_| "-46,69,12".into());
    let parts: Vec<i32> = text.split(',').map(|p| p.trim().parse().unwrap()).collect();
    let goal = [parts[0], parts[1], parts[2]];
    let mut nav = NavService::new(Budget::Unlimited);
    let mut found = Vec::new();
    for dx in -7..=7 {
        for dz in -7..=7 {
            for dy in -2..=4 {
                let cell = [goal[0] + dx, goal[1] + dy, goal[2] + dz];
                if let Some(stand) = nav.stand(&world, cell) {
                    let floor = world.block_at(cell[0], cell[1] - 1, cell[2]).unwrap_or(0);
                    let name = names[&u32::from(floor)].split('[').next().unwrap();
                    found.push((dx * dx + dz * dz, cell, stand.feet_y, name));
                }
            }
        }
    }
    found.sort_by_key(|(d, cell, ..)| (*d, cell[1]));
    let mut seen = std::collections::HashSet::new();
    for (_, cell, feet, floor) in found {
        // One line per kind of floor and height keeps the list readable.
        if seen.insert((floor, (feet * 2.0) as i32, cell[0] / 2, cell[2] / 2)) {
            eprintln!("  {cell:?} feet={feet} floor={floor}");
        }
    }
}

/// A headless bot dropped at `HALCYON_SURVEY_AT`, traced as it recovers.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn trace_bot() {
    let world = load_world();
    let text = std::env::var("HALCYON_SURVEY_AT").unwrap_or_else(|_| "-27.5,58.2,-36.5".into());
    let parts: Vec<f64> = text.split(',').map(|p| p.trim().parse().unwrap()).collect();
    let at = Transform { x: parts[0], y: parts[1], z: parts[2], ..Transform::default() };
    let mut nav = NavService::new(Budget::Unlimited);
    let goal = std::env::var("HALCYON_TRACE_GOAL").ok().map(|text| {
        let parts: Vec<i32> = text.split(',').map(|p| p.trim().parse().unwrap()).collect();
        [parts[0], parts[1], parts[2]]
    });
    nav.goal = Some(goal.unwrap_or([-46, 69, 12]));
    let level = env_or("HALCYON_TRACE_LEVEL", 100) as u8;
    let every = env_or("HALCYON_TRACE_EVERY", 10);
    let mut bot = SimBot::new(BotProfile::new(level), 1, at);
    if std::env::var("HALCYON_BOT_TRACE").is_ok() {
        bot.brain.traced = Some("sim".into());
    }
    let mut arena = settled(&world);
    for tick in 0..env_or("HALCYON_BENCH_TICKS", 600) {
        step_with_doors(&mut bot, &mut nav, &mut arena, &world);
        if tick % every == 0 {
            let p = bot.position();
            eprintln!(
                "t={tick} mode={:?} ({:.2},{:.2},{:.2}) ground={}",
                bot.brain.mode(),
                p.x,
                p.y,
                p.z,
                p.on_ground
            );
        }
    }
}

/// Success rate of one edge under the live controller and motor at a level,
/// against its validated robustness. Exposes validation/runtime mismatches.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn edge_success_rate() {
    use super::follow::{EdgeRun, Execution, Obs, Status};
    let world = load_world();
    let parse = |name: &str, default: &str| {
        let text = std::env::var(name).unwrap_or_else(|_| default.into());
        let parts: Vec<i32> = text.split(',').map(|p| p.trim().parse().unwrap()).collect();
        [parts[0], parts[1], parts[2]]
    };
    let from = parse("HALCYON_TRACE_FROM", "-13,61,-26");
    let to = parse("HALCYON_TRACE_TO", "-13,62,-30");
    let level = env_or("HALCYON_TRACE_LEVEL", 100) as u8;
    let mut nav = NavService::new(Budget::Unlimited);
    let here = nav.stand(&world, from).expect("from is standable");
    let candidate = *nav
        .graph
        .candidates(&world, from)
        .expect("candidates")
        .iter()
        .find(|c| c.to == to)
        .expect("a candidate");
    let edge = nav.graph.validate(&world, from, &candidate).expect("a valid edge");
    let profile = BotProfile::new(level);
    let mut successes = 0;
    let trials = 20;
    for seed in 0..trials {
        let mut rng = crate::bot::rng::Rng::new(seed);
        let mut motor = crate::bot::motor::Motor::new(0.0, 0.0);
        let exec = Execution {
            aim_tolerance: profile.aim_tolerance(),
            approach_tolerance: profile.approach_tolerance(),
            hesitation: profile.hesitation(edge.robustness, &mut rng),
            ..Execution::IDEAL
        };
        let mut run = EdgeRun::new(from, &here, edge, exec);
        let [x, z] = super::stand::centre(from);
        let start = Transform { x, y: here.feet_y, z, on_ground: true, ..Transform::default() };
        let mut state = crate::movement::MovementState::new(start);
        state.place(start);
        let mut previous = start;
        let mut velocity = [0.0; 3];
        for _ in 0..200 {
            let position = state.transform();
            let (intent, status) = run.tick(&Obs { position, velocity });
            match status {
                Status::Running => {}
                Status::Done => {
                    successes += 1;
                    break;
                }
                Status::Failed(_) => break,
            }
            state.step(&world, motor.tick(intent, profile, &mut rng));
            let now = state.transform();
            velocity = [now.x - previous.x, now.y - previous.y, now.z - previous.z];
            previous = now;
        }
    }
    eprintln!(
        "edge {from:?} -> {to:?} plan={:?} robustness={} level={level}: {successes}/{trials}",
        edge.plan, edge.robustness
    );
}

/// Validated route search for every leg between consecutive checkpoints:
/// which legs bots can complete and where the others stop.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn checkpoint_legs() {
    let world = load_world();
    let names = block_names();
    let (x, y, z, _, _) = world.spawn;
    let around = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
    let mut nav = NavService::new(Budget::Unlimited);
    let checkpoints = nav.detect_checkpoints(&world, around, 128);
    let goal = nav.detect_goal(&world, around, 128).expect("a goal");
    let stops: Vec<Cell> = checkpoints.iter().copied().chain(std::iter::once(goal)).collect();
    for (index, pair) in stops.windows(2).enumerate() {
        let started = std::time::Instant::now();
        let mut search = Search::default();
        search.start(pair[0], pair[1]);
        let status = loop {
            match search.step(&mut nav, &OpenDoors(&world), 100_000, &|_, _| 0.0) {
                SearchStatus::Running => {}
                done => break done,
            }
        };
        let summary = match &status {
            SearchStatus::Found(path) => {
                let long: Vec<String> = path
                    .iter()
                    .filter(|s| {
                        let d = f64::from(s.edge.to[0] - s.from[0])
                            .hypot(f64::from(s.edge.to[2] - s.from[2]));
                        s.edge.plan.runup > 1.0 || d >= 4.0
                    })
                    .map(|s| {
                        format!(
                            "{:?}->{:?} runup={} hop={} robustness={:.2}",
                            s.from,
                            s.edge.to,
                            s.edge.plan.runup,
                            s.edge.plan.hop,
                            s.edge.robustness
                        )
                    })
                    .collect();
                format!("found {} steps; long jumps: {long:?}", path.len())
            }
            SearchStatus::Partial(path) => {
                let end = path.last().map(|s| s.edge.to);
                // Mechanics near where the route stops hint at what is missing.
                let mut near = std::collections::BTreeSet::new();
                if let Some([ex, ey, ez]) = end {
                    for x in ex - 4..=ex + 4 {
                        for y in ey - 3..=ey + 5 {
                            for z in ez - 4..=ez + 4 {
                                let id = world.block_at(x, y, z).unwrap_or(0);
                                let name = names[&u32::from(id)].split('[').next().unwrap();
                                let name = name.trim_start_matches("minecraft:");
                                if [
                                    "door",
                                    "trapdoor",
                                    "ladder",
                                    "vine",
                                    "bubble",
                                    "scaffold",
                                    "slime",
                                    "button",
                                    "lever",
                                    "piston",
                                    "water",
                                    "honey",
                                    "fence_gate",
                                ]
                                .iter()
                                .any(|word| name.contains(word))
                                {
                                    near.insert(name.to_owned());
                                }
                            }
                        }
                    }
                }
                format!("PARTIAL, stops at {end:?} near {near:?}")
            }
            _ => "UNREACHABLE".to_owned(),
        };
        eprintln!(
            "leg {} -> {}: {:?} -> {:?} {summary} ({} ms)",
            index + 1,
            index + 2,
            pair[0],
            pair[1],
            started.elapsed().as_millis()
        );
    }
}

/// Why lily pads near spawn are or are not navigation nodes.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_lily_pads() {
    let world = load_world();
    let names = block_names();
    let pad = halcyon_world::blockstates::block_state_id("minecraft:lily_pad").unwrap() as u16;
    let info = halcyon_world::motion_info(pad);
    eprintln!(
        "lily pad: surface={:?} boxes={:?} contextual={}",
        info.surface,
        halcyon_world::state_info(pad).collision_boxes(),
        halcyon_world::contextual_geometry(pad)
    );
    let mut nav = NavService::new(Budget::Unlimited);
    let mut shown = 0;
    for z in -95..-55 {
        for x in -20..20 {
            for y in 50..70 {
                if world.block_at(x, y, z) == Some(pad) && shown < 6 {
                    shown += 1;
                    let below = world.block_at(x, y - 1, z).map(|id| names[&u32::from(id)]);
                    let stand = nav.stand(&world, [x, y, z]);
                    let edges = nav.graph.edges(&world, [x, y, z]);
                    eprintln!(
                        "pad at {:?} below={below:?} stand={stand:?} edges={:?}",
                        [x, y, z],
                        edges.map(|e| e
                            .iter()
                            .map(|e| (e.to, e.kind, e.robustness))
                            .collect::<Vec<_>>())
                    );
                }
            }
        }
    }
}

/// Tick-by-tick trace of one edge execution, for debugging validation.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn trace_edge() {
    use super::follow::{EdgeRun, Execution, Obs, Status};
    let world = load_world();
    let arena = settled(&world);
    let view = arena.view(&world);
    let cell = |name: &str, default: [i32; 3]| {
        std::env::var(name)
            .ok()
            .and_then(|at| {
                let mut parts = at.split(',').map(|p| p.trim().parse::<i32>().ok());
                Some([parts.next()??, parts.next()??, parts.next()??])
            })
            .unwrap_or(default)
    };
    let from = cell("HALCYON_TRACE_FROM", [0, 52, -78]);
    let to = cell("HALCYON_TRACE_TO", [-2, 52, -77]);
    let mut nav = NavService::new(Budget::Unlimited);
    let here = nav.stand(&view, from).expect("from is standable");
    let there = nav.stand(&view, to).expect("to is standable");
    // Trace the graph's own candidate for this pair when there is one.
    let candidate = nav
        .graph
        .candidates(&view, from)
        .and_then(|found| found.iter().find(|c| c.to == to).copied());
    eprintln!("candidate {candidate:?}");
    let (kind, via) = candidate.map_or((EdgeKind::Jump, None), |c| (c.kind, c.via));
    let runup: f32 = std::env::var("HALCYON_TRACE_RUNUP").map_or(0.3, |r| r.parse().unwrap());
    let takeoff: f32 = std::env::var("HALCYON_TRACE_TAKEOFF").map_or(0.6, |r| r.parse().unwrap());
    let sprint = std::env::var("HALCYON_TRACE_WALK").is_err();
    for (runup, takeoff, sprint) in [(runup, takeoff, sprint)] {
        let edge = Edge {
            to,
            to_feet: there.feet_y,
            kind,
            plan: Plan { runup, takeoff, sprint, hop: false, lean: [0.0; 2] },
            ticks: 0,
            robustness: 0.0,
            via,
            damage: 0.0,
        };
        let mut run = EdgeRun::new(from, &here, edge, Execution::IDEAL);
        let [x, z] = super::stand::centre(from);
        let start = Transform { x, y: here.feet_y, z, on_ground: true, ..Transform::default() };
        let mut state = crate::movement::MovementState::new(start);
        state.place(start);
        let mut previous = start;
        let mut velocity = [0.0; 3];
        eprintln!("plan runup={runup} takeoff={takeoff} sprint={sprint}");
        for tick in 0..80 {
            let position = state.transform();
            let (intent, status) = run.tick(&Obs { position, velocity });
            let advance = state.step(&view, intent.exact());
            let now = state.transform();
            eprintln!(
                "  t={tick} pos=({:.3},{:.3},{:.3}) ground={} status={status:?} fwd={} back={} jump={} sprint={} correction={} uncertain={} collisions={:?}",
                position.x,
                position.y,
                position.z,
                position.on_ground,
                intent.forward,
                intent.backward,
                intent.jump,
                intent.sprint,
                advance.correction,
                advance.uncertain_motion,
                advance.collisions
            );
            if status != Status::Running {
                break;
            }
            velocity = [now.x - previous.x, now.y - previous.y, now.z - previous.z];
            previous = now;
        }
    }
}

/// Optimistic route from spawn to goal over unvalidated candidates, then
/// validation of each step along it: shows which moves the map really needs.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn geometric_route() {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};
    let world = load_world();
    let names = block_names();
    let name = |id: Option<u16>| {
        id.and_then(|id| names.get(&u32::from(id)).copied())
            .unwrap_or("-")
            .trim_start_matches("minecraft:")
            .split('[')
            .next()
            .unwrap()
            .to_owned()
    };
    let (x, y, z, _, _) = world.spawn;
    let mut settle =
        crate::movement::MovementState::new(Transform { x, y, z, ..Transform::default() });
    for _ in 0..40 {
        settle.step(&world, crate::movement::MovementInput::default());
    }
    // HALCYON_ROUTE_FROM / HALCYON_ROUTE_TO ("x,y,z") examine one leg, such
    // as between two checkpoints.
    let cell = |name: &str| {
        let text = std::env::var(name).ok()?;
        let mut parts = text.split(',').map(|p| p.trim().parse::<i32>().ok());
        Some([parts.next()??, parts.next()??, parts.next()??])
    };
    let start = cell("HALCYON_ROUTE_FROM").unwrap_or(super::stand::cell_of(settle.transform()));
    let mut nav = NavService::new(Budget::Unlimited);
    let goal =
        cell("HALCYON_ROUTE_TO").or_else(|| nav.detect_goal(&world, start, 128)).expect("goal");
    let started = std::time::Instant::now();
    // Dijkstra-ish weighted search over candidate estimates.
    let mut open = BinaryHeap::new();
    let mut best: HashMap<Cell, (u32, Option<(Cell, Candidate)>)> = HashMap::new();
    let key = |g: f32, cell: Cell| {
        (g * 10.0) as u32 + (super::search::heuristic(cell, goal) * 20.0) as u32
    };
    open.push(Reverse((key(0.0, start), 0u32, start)));
    best.insert(start, (0, None));
    let mut expanded = 0usize;
    let mut reached = None;
    let mut closest = (u32::MAX, start);
    while let Some(Reverse((_, g10, cell))) = open.pop() {
        if best.get(&cell).is_some_and(|(known, _)| *known < g10) {
            continue;
        }
        expanded += 1;
        let h = (super::search::heuristic(cell, goal) * 10.0) as u32;
        if h < closest.0 {
            closest = (h, cell);
        }
        if cell == goal {
            reached = Some(cell);
            break;
        }
        if expanded > 2_000_000 {
            break;
        }
        let Some(candidates) = nav.graph.candidates(&world, cell) else {
            continue;
        };
        for candidate in candidates.iter() {
            let g = g10 + (candidate.estimate * 10.0) as u32 + 80;
            if best.get(&candidate.to).is_none_or(|(known, _)| g < *known) {
                best.insert(candidate.to, (g, Some((cell, *candidate))));
                open.push(Reverse((key(g as f32 / 10.0, candidate.to), g, candidate.to)));
            }
        }
    }
    let end = reached.unwrap_or(closest.1);
    // Near misses: unreached floors right next to reached ones, higher up.
    let mut misses: Vec<(i32, Cell, Cell)> = Vec::new();
    let floor = env_or("HALCYON_MISS_FLOOR", 0) as i32;
    for from in best
        .keys()
        .filter(|c| (c[0] - end[0]).abs() <= 24 && (c[2] - end[2]).abs() <= 24 && c[1] >= floor)
    {
        for dx in -2..=2 {
            for dz in -2..=2 {
                for dy in 1..=6 {
                    let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                    if !best.contains_key(&to) && nav.stand(&world, to).is_some() {
                        misses.push((to[1], *from, to));
                    }
                }
            }
        }
    }
    misses.sort();
    misses.dedup_by_key(|(_, _, to)| *to);
    eprintln!("near misses (lowest first):");
    for (_, from, to) in misses.iter().take(env_or("HALCYON_MISS_COUNT", 25) as usize) {
        let column: Vec<String> =
            (from[1] - 1..=to[1]).map(|y| name(world.block_at(to[0], y, to[2]))).collect();
        eprintln!("  {from:?} -> {to:?} column below target: {column:?}");
    }
    // Highest standable cell per column near the end: digit = y % 10 if the
    // search reached it, letter (a = 0) if not.
    let radius = env_or("HALCYON_FRONTIER_RADIUS", 14) as i32;
    let centre = cell("HALCYON_MAP_AT").unwrap_or(end);
    eprintln!("geometric map around {centre:?} (x left to right, z top to bottom):");
    for z in centre[2] - radius..=centre[2] + radius {
        let mut row = String::new();
        for x in centre[0] - radius..=centre[0] + radius {
            let heights = (centre[1] - 8..=centre[1] + 15).rev();
            let reached = heights.clone().find(|&y| best.contains_key(&[x, y, z]));
            let found = reached.map(|y| ([x, y, z], true)).or_else(|| {
                heights
                    .clone()
                    .find(|&y| nav.stand(&world, [x, y, z]).is_some())
                    .map(|y| ([x, y, z], false))
            });
            row.push(match found {
                None => '.',
                Some((cell, _)) if cell == end => '@',
                Some((cell, reached)) => {
                    let digit = cell[1].rem_euclid(10) as u8;
                    char::from(if reached { b'0' } else { b'a' } + digit)
                }
            });
        }
        eprintln!("  {row}");
    }
    eprintln!(
        "geometric: reached_goal={} expanded={expanded} end={end:?} goal={goal:?} in {}ms nodes={}",
        reached.is_some(),
        started.elapsed().as_millis(),
        nav.stats().nodes
    );
    let mut path = Vec::new();
    let mut cell = end;
    while let Some((_, Some((from, candidate)))) = best.get(&cell) {
        path.push((*from, *candidate));
        cell = *from;
    }
    path.reverse();
    let mut failures = 0;
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for (index, (from, candidate)) in path.iter().enumerate() {
        let valid = nav.graph.validate(&world, *from, candidate).is_some();
        if !valid {
            failures += 1;
            let to = candidate.to;
            let floor = name(world.block_at(to[0], to[1] - 1, to[2]));
            let here = name(world.block_at(from[0], from[1] - 1, from[2]));
            *kinds.entry(format!("{:?} {here} -> {floor}", candidate.kind)).or_default() += 1;
            if failures <= 25 {
                eprintln!(
                    "  step {index}: {:?} {from:?} ({here}) -> {to:?} ({floor}) d={:.2} rise={:.2}",
                    candidate.kind,
                    f64::from(to[0] - from[0]).hypot(f64::from(to[2] - from[2])),
                    candidate.to_feet - nav.stand(&world, *from).map_or(0.0, |s| s.feet_y),
                );
            }
        }
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
    eprintln!(
        "geometric path: {} steps, {failures} fail validation; by kind: {kinds:?}",
        path.len()
    );
    let top = path.iter().map(|(_, c)| c.to[1]).max().unwrap_or(0);
    eprintln!("geometric path top y={top}");
}

/// Block types around a point, flagging those navigation treats as hazards.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn survey_inventory() {
    let world = load_world();
    let names = block_names();
    let at = std::env::var("HALCYON_SURVEY_AT").unwrap_or_else(|_| "0,54,-45".into());
    let mut parts = at.split(',').map(|p| p.trim().parse::<i32>().unwrap());
    let [cx, cy, cz] = [parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap()];
    let radius = env_or("HALCYON_SURVEY_RADIUS", 30) as i32;
    let height = env_or("HALCYON_SURVEY_LAYERS", 70) as i32;
    let mut counts: std::collections::HashMap<String, (usize, [i32; 3])> = Default::default();
    for x in cx - radius..=cx + radius {
        for z in cz - radius..=cz + radius {
            for y in cy..cy + height {
                let Some(id) = world.block_at(x, y, z) else { continue };
                if id == 0 {
                    continue;
                }
                let base = names[&u32::from(id)]
                    .split('[')
                    .next()
                    .unwrap()
                    .trim_start_matches("minecraft:");
                // HALCYON_SURVEY_LIST: every block of a kind, by position.
                if std::env::var("HALCYON_SURVEY_LIST").is_ok_and(|kind| kind == base) {
                    eprintln!("  {base} at [{x}, {y}, {z}]");
                }
                let unsupported =
                    halcyon_world::motion_info(id).surface == halcyon_world::Surface::Unsupported;
                let contextual = halcyon_world::contextual_geometry(id);
                let tag = format!(
                    "{base}{}{}",
                    if unsupported { " UNSUPPORTED" } else { "" },
                    if contextual { " CONTEXTUAL" } else { "" }
                );
                let entry = counts.entry(tag).or_insert((0, [x, y, z]));
                entry.0 += 1;
            }
        }
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    if let Ok(filter) = std::env::var("HALCYON_SURVEY_FILTER") {
        let words: Vec<&str> = filter.split(',').collect();
        counts.retain(|(tag, _)| words.iter().any(|w| tag.contains(w)));
    }
    for (tag, (count, example)) in counts.iter().take(60) {
        eprintln!("  {count:>7} {tag} e.g. {example:?}");
    }
}

/// Where the explored graph stops: the highest rejected moves and the nearest
/// standable cells above the explored area that no candidate reaches.
fn frontier_report(nav: &mut NavService, world: &halcyon_world::World, end: Cell) {
    use std::collections::HashSet;
    let names = block_names();
    let name = |id: Option<u16>| {
        id.and_then(|id| names.get(&u32::from(id)).copied())
            .unwrap_or("-")
            .trim_start_matches("minecraft:")
    };
    let near = |c: &Cell| (c[0] - end[0]).pow(2) + (c[2] - end[2]).pow(2) <= 25 * 25;
    let explored: HashSet<Cell> =
        nav.graph.validated.keys().map(|(from, ..)| *from).filter(near).collect();
    let mut rejected: Vec<(Cell, Cell)> = nav
        .graph
        .validated
        .iter()
        .filter(|(_, edge)| edge.is_none())
        .map(|(&(from, to, _), _)| (from, to))
        .filter(|(from, to)| near(from) && to[1] > from[1] && !explored.contains(to))
        .collect();
    rejected.sort_by_key(|(_, to)| -to[1]);
    eprintln!("frontier: {} explored cells; highest rejected upward moves:", explored.len());
    for (from, to) in rejected.iter().take(20) {
        eprintln!(
            "  {from:?} -> {to:?} target_floor={} target_feet={}",
            name(world.block_at(to[0], to[1] - 1, to[2])),
            name(world.block_at(to[0], to[1], to[2]))
        );
    }
    // Local map: highest standable cell per column; digit = feet y % 10 if
    // the search reached it, letter (a = 0) if it did not.
    let radius = env_or("HALCYON_FRONTIER_RADIUS", 14) as i32;
    eprintln!("frontier map around {end:?} (x left to right, z top to bottom):");
    for z in end[2] - radius..=end[2] + radius {
        let mut row = String::new();
        for x in end[0] - radius..=end[0] + radius {
            let found = (end[1] - 4..=end[1] + 20)
                .rev()
                .find(|&y| nav.stand(world, [x, y, z]).is_some())
                .map(|y| [x, y, z]);
            row.push(match found {
                None => '.',
                Some(cell) if cell == end => '@',
                Some(cell) => {
                    let digit = cell[1].rem_euclid(10) as u8;
                    if nav.graph.candidates.contains_key(&cell) {
                        char::from(b'0' + digit)
                    } else {
                        char::from(b'a' + digit)
                    }
                }
            });
        }
        eprintln!("  {row}");
    }
    let top = explored.iter().map(|c| c[1]).max().unwrap_or(0);
    let mut unreached: Vec<(i32, Cell, Cell)> = Vec::new();
    for from in explored.iter().filter(|c| c[1] >= top - 3) {
        for dx in -6..=6 {
            for dz in -6..=6 {
                for dy in 1..=6 {
                    let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                    let tried = nav.graph.validated.keys().any(|&(a, b, _)| a == *from && b == to);
                    if explored.contains(&to) || tried {
                        continue;
                    }
                    if nav.stand(world, to).is_some() {
                        unreached.push((dx * dx + dz * dz + dy * dy, *from, to));
                    }
                }
            }
        }
    }
    unreached.sort();
    unreached.dedup_by_key(|(_, _, to)| *to);
    eprintln!("frontier: nearest standable cells above, never proposed as candidates:");
    for (_, from, to) in unreached.iter().take(20) {
        eprintln!(
            "  {from:?} -> {to:?} floor={} feet={} head={}",
            name(world.block_at(to[0], to[1] - 1, to[2])),
            name(world.block_at(to[0], to[1], to[2])),
            name(world.block_at(to[0], to[1] + 1, to[2]))
        );
    }
}

/// One bot tick on an editable map: hand uses toggle doors as the lobby
/// owner would.
fn step_with_doors(
    bot: &mut SimBot,
    nav: &mut NavService,
    arena: &mut crate::arena::Arena,
    world: &halcyon_world::World,
) {
    bot.step(nav, &arena.view(world));
    if let Some(pos) = bot.brain.take_use() {
        let cells = crate::interaction::toggled_cells(&arena.view(world), pos, bot.position().yaw);
        if let Some(cells) = cells {
            let _ = arena.edit_all(world, &cells);
            nav.sync(arena);
        }
    }
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
    nav.checkpoints = nav.detect_checkpoints(&world, around, 128);
    eprintln!("checkpoints: {:?}", nav.checkpoints);
    eprintln!("spawn {spawn:?} goal {goal:?} detected in {}ms", started.elapsed().as_millis());

    // Route existence from spawn, independent of execution quality.
    let started = std::time::Instant::now();
    let mut search = Search::default();
    // The recorded spawn may hover above its floor: let it land first.
    let mut settle = crate::movement::MovementState::new(spawn);
    for _ in 0..40 {
        settle.step(&world, crate::movement::MovementInput::default());
    }
    let start = super::stand::cell_of(settle.transform());
    search.start(start, goal);
    let route = loop {
        match search.step(&mut nav, &OpenDoors(&world), 100_000, &|_, _| 0.0) {
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
    if let SearchStatus::Partial(path) = &route
        && let Some(end) = path.last().map(|s| s.edge.to)
    {
        frontier_report(&mut nav, &world, end);
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
    let mut arena = settled(&world);
    for tick in 1..=limit {
        let tick_started = std::time::Instant::now();
        for bot in &mut bots {
            step_with_doors(bot, &mut nav, &mut arena, &world);
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
            "bot level={} mode={:?} checkpoint={} max_y={:.1} y={:.1} finished_at={:?} falls={} deaths={} edges={}",
            bot.brain.profile.level,
            bot.brain.mode(),
            bot.brain.checkpoint(),
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

/// Every jump plan between two cells on the settled terrain, with outcomes.
#[test]
#[ignore = "opt-in survey of a real map for navigation work"]
fn jump_grid() {
    let world = load_world();
    let arena = settled(&world);
    let view = arena.view(&world);
    let parse = |name: &str, default: &str| {
        let text = std::env::var(name).unwrap_or_else(|_| default.into());
        let parts: Vec<i32> = text.split(',').map(|p| p.trim().parse().unwrap()).collect();
        [parts[0], parts[1], parts[2]]
    };
    let from = parse("HALCYON_TRACE_FROM", "-21,90,-43");
    let to = parse("HALCYON_TRACE_TO", "-21,90,-38");
    let mut nav = NavService::new(Budget::Unlimited);
    let here = nav.stand(&view, from).expect("from is standable");
    let there = nav.stand(&view, to).expect("to is standable");
    let [ax, az] = super::stand::centre(from);
    let [bx, bz] = super::stand::centre(to);
    let distance = (bx - ax).hypot(bz - az);
    let dir = [(bx - ax) / distance, (bz - az) / distance];
    let rim = here.rim(dir) as f32;
    let back = here.rim([-dir[0], -dir[1]]) as f32;
    let runway = nav.graph.runway(&view, from, here, dir, back);
    eprintln!("rim {rim} back {back} runway {runway} distance {distance}");
    let candidate = nav
        .graph
        .candidates(&view, from)
        .and_then(|found| found.iter().find(|c| c.to == to).copied());
    eprintln!("candidate {candidate:?}");
    if let Some(all) = nav.graph.candidates(&view, from) {
        let jumps: Vec<_> =
            all.iter().filter(|c| c.kind == EdgeKind::Jump).map(|c| (c.to, c.estimate)).collect();
        eprintln!("{} candidates, {} jumps: {jumps:?}", all.len(), jumps.len());
    }
    eprintln!("validated {:?}", candidate.and_then(|c| nav.graph.validate(&view, from, &c)));
    let hop = std::env::var("HALCYON_TRACE_HOP").is_ok();
    for runup in (0..=60).map(|k| k as f32 * 0.05).filter(|&r| r <= runway + 0.01) {
        for takeoff in [rim + 0.2, rim + 0.29] {
            let edge = Edge {
                to,
                to_feet: there.feet_y,
                kind: EdgeKind::Jump,
                plan: Plan { runup, takeoff, sprint: true, hop, lean: [0.0; 2] },
                ticks: 0,
                robustness: 0.0,
                via: None,
                damage: 0.0,
            };
            let out =
                nav.graph.simulate(&view, from, here, edge, super::follow::Execution::IDEAL, false);
            eprintln!(
                "runup {runup:.1} takeoff {takeoff:.2}: ticks {:?} closest {:.2} furthest {:.2}",
                out.ticks, out.closest, out.furthest
            );
        }
    }
}
