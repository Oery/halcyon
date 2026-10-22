//! Opt-in production-rule workloads: terrain, owner steps, 20 Hz publication,
//! ground physics, equipment and encrypted bounded writers. No sockets.
use super::*;
use crate::inventory::{HotbarSlot, InventoryAction};
use crate::lifecycle::Damage;
use crate::movement::MovementProposal;
use crate::player::PlayerAction;
use crate::simulation::SimulationClock;
use futures_lite::future::block_on;
use glommio::timer::Timer;
use halcyon_world::{
    LoadedWorld, Section, World, blockstates::block_state_id, loader::LoadedChunk,
};
use mc_protocol::packets::{Payload, client::PlayerInfoUpdate};
use serde_json::{Value, json};
use std::time::Instant;

mod receiver;

use receiver::Receiver;

const PLAYERS: usize = 100;
const WARMUP: usize = 20;
const SAMPLES: usize = 1200;
const DEADLINE_NS: u128 = 1_000_000_000 / crate::simulation::SIMULATION_HZ as u128;
// Fast writers remain runnable after a bounded slice. Model successive fair
// writer turns independently of the owner's simulation cadence.
const WRITER_ROUNDS: usize = 16;

pub(super) fn terrain(spread: bool, steps: bool) -> World {
    make_terrain(spread, steps, false, Medium::Dry)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Medium {
    Dry,
    Water,
    Crawl,
}

fn make_terrain(spread: bool, steps: bool, surfaces: bool, medium: Medium) -> World {
    let stone = block_state_id("minecraft:stone").unwrap() as u16;
    let slab =
        block_state_id("minecraft:stone_slab[type=bottom,waterlogged=false]").unwrap() as u16;
    let columns: Vec<_> = if spread {
        (0..PLAYERS as i32).map(|id| (id * 4, 0)).collect()
    } else {
        (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect()
    };
    let chunks = columns
        .into_iter()
        .map(|(x, z)| {
            let mut floor = Box::new([0; 4096]);
            floor[15 * 256..].fill(stone);
            if surfaces {
                let surfaces = [
                    "minecraft:ice",
                    "minecraft:soul_sand",
                    "minecraft:honey_block",
                    "minecraft:slime_block",
                ]
                .map(|key| block_state_id(key).unwrap() as u16);
                for z in 0..16 {
                    for x in 0..16 {
                        floor[15 * 256 + z * 16 + x] = surfaces[x / 4];
                    }
                }
            }
            let mut sections = Box::new(std::array::from_fn(|_| Section::default()));
            sections[7] = Section::from_states(floor);
            if steps {
                let mut layer = Box::new([0; 4096]);
                layer[8 * 16 + 9] = slab;
                sections[8] = Section::from_states(layer);
            }
            if medium != Medium::Dry {
                let water = block_state_id("minecraft:water[level=0]").unwrap() as u16;
                let mut layer = Box::new([0; 4096]);
                if medium == Medium::Water {
                    layer[..8 * 256].fill(water);
                } else {
                    layer[256..512].fill(stone);
                }
                sections[8] = Section::from_states(layer);
            }
            LoadedChunk { x, z, data: vec![], heights: [0; 37], wire_bytes: 0, sections }
        })
        .collect();
    World::from_loaded(LoadedWorld {
        chunks,
        spawn: (8.5, 64.0, 8.5, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    })
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    spread: bool,
    input_period: usize,
    steps: bool,
    churn: bool,
    driven: bool,
    falling: bool,
    medium: Medium,
}

async fn measure(case: Scenario, paced: bool) -> Value {
    let world = make_terrain(
        case.spread,
        case.steps,
        case.driven && case.medium == Medium::Dry,
        case.medium,
    );
    let mut registry = Registry::default();
    let mut inputs = Vec::with_capacity(PLAYERS);
    let mut receivers = Vec::with_capacity(PLAYERS);
    for id in 0..PLAYERS {
        let name = format!("bench{id}");
        let uuid = Uuid::from_u128(id as u128 + 1);
        let mut info = Vec::new();
        PlayerInfoUpdate { uuid, username: &name, properties: vec![] }
            .write_packet(&mut info)
            .await
            .unwrap();
        let input = Rc::new(Input::new(Rc::default()));
        let transform = Transform {
            x: if case.spread {
                id as f64 * 64.0 + 8.5
            } else if case.driven {
                3.5 + (id % 4) as f64 * 3.0
            } else {
                8.5
            },
            y: if case.medium == Medium::Water { 66.0 } else { 64.0 },
            z: 8.5,
            ..Default::default()
        };
        let entity_id = registry
            .reserve(
                Rc::new(Profile { uuid, username: name, info: Rc::from(info) }),
                transform,
                0x7f,
                input.clone(),
            )
            .unwrap();
        input.activate.set(true);
        let player = registry.players.get_mut(&entity_id).unwrap();
        player.state.life.rules.protected = !case.churn;
        player.state.life.rules.respawn = transform;
        player
            .state
            .inventory
            .set_slot(HotbarSlot::FIRST.inventory_slot(), ItemStack::plain(1, 32).unwrap())
            .unwrap();
        player
            .state
            .inventory
            .set_slot(
                HotbarSlot::try_from(1usize).unwrap().inventory_slot(),
                ItemStack::plain(2, 32).unwrap(),
            )
            .unwrap();
        player.state.equipment = Rc::new(player.state.inventory.equipment());
        inputs.push((entity_id, input, transform));
        let terrain = world
            .iter()
            .filter(|chunk| {
                (chunk.x - transform.chunk().0).abs() <= 8
                    && (chunk.z - transform.chunk().1).abs() <= 8
            })
            .map(|chunk| (chunk.x, chunk.z))
            .collect();
        receivers.push(Receiver::new(terrain));
    }
    let mut notices = Vec::new();
    for _ in 0..120 {
        if registry.publish() {
            registry.revision += 1;
        }
        for ((id, _, _), receiver) in inputs.iter().zip(&mut receivers) {
            receiver.observer.reconcile(&registry, *id, &receiver.terrain).unwrap();
            receiver.drain(&registry);
        }
    }
    assert!(receivers.iter().all(|receiver| receiver.observer.queued_frames() == 0));
    let origin = Instant::now();
    let mut clock = SimulationClock::new(origin);
    let samples = if paced { 200 } else { SAMPLES };
    let mut combined = Vec::with_capacity(samples);
    let mut simulation = Vec::with_capacity(samples);
    let mut lateness = Vec::with_capacity(samples);
    let mut skipped = 0;
    let mut corrections = 0;
    let mut cells = 0;
    let mut swimming_steps = 0;
    let mut crawling_steps = 0;
    let mut queue_peak = 0;
    let mut peak_pending_motions = 0;
    let mut max_revision_lag = 0;
    for tick in 0..WARMUP + samples {
        let due_at = clock.next_deadline();
        if paced {
            Timer::new(due_at.saturating_duration_since(Instant::now())).await;
        }
        let due = clock.poll(if paced { Instant::now() } else { due_at });
        skipped += due.skipped_steps;
        let start = Instant::now();
        let now = if paced { start } else { due_at };
        if case.falling && tick % 40 == 0 {
            for index in 0..32 {
                let x = 12 + index % 4;
                let z = 1 + index / 4;
                registry.arena.edit(&world, [x, 64, z], 0).unwrap();
                registry
                    .arena
                    .edit(&world, [x, 70, z], if index % 2 == 0 { 121 } else { 127 })
                    .unwrap();
            }
        }
        if case.input_period != 0 && tick % case.input_period == 0 {
            for (id, input, base) in &inputs {
                let mut transform = *base;
                if case.steps {
                    if tick % 4 == 1 {
                        transform.x += 0.75;
                    }
                    if matches!(tick % 4, 1 | 2) {
                        transform.y += 0.5;
                    }
                } else {
                    transform.x += (tick as f64 / crate::simulation::SIMULATION_HZ as f64
                        * std::f64::consts::TAU)
                        .sin()
                        * 0.25;
                }
                transform.yaw = tick as f32;
                if case.driven {
                    input.actions.borrow_mut().push(
                        input.generation.get(),
                        PlayerAction::Drive(Some(crate::movement::MovementInput {
                            forward: true,
                            yaw: tick as f32 * 12.0,
                            jumping: tick % 40 == 0,
                            sprinting: true,
                            ..Default::default()
                        })),
                    );
                } else {
                    input.proposals.borrow_mut().push(MovementProposal {
                        transform,
                        sneaking: false,
                        sprinting: true,
                    });
                }
                if case.churn && tick % 5 == 0 {
                    input.actions.borrow_mut().push(
                        input.generation.get(),
                        PlayerAction::Inventory(InventoryAction::Select {
                            slot: HotbarSlot::try_from((tick / 5) % 2).unwrap(),
                        }),
                    );
                    let action = if !registry.players[id].state.life.alive() {
                        PlayerAction::Respawn
                    } else {
                        PlayerAction::Damage(Damage {
                            kind: crate::lifecycle::DamageKind::Game,
                            amount: if tick > 0 && tick % 100 == 0 { 100.0 } else { 1.0 },
                            source: None,
                        })
                    };
                    input.actions.borrow_mut().push(input.generation.get(), action);
                    input
                        .actions
                        .borrow_mut()
                        .push(input.generation.get(), PlayerAction::Heal(1.0));
                }
            }
        }
        let simulation_start = Instant::now();
        for _ in 0..due.steps {
            let metrics = registry.simulate(&world, now, &mut notices);
            cells += metrics.collision_cells;
            corrections += metrics.corrections;
            if case.medium != Medium::Dry {
                for player in registry.players.values() {
                    swimming_steps += usize::from(player.state.movement.swimming());
                    crawling_steps += usize::from(
                        player.state.movement.pose() == crate::movement::PlayerPose::Swimming
                            && !player.state.movement.swimming(),
                    );
                }
            }
            notices.clear();
        }
        let sim_ns = simulation_start.elapsed().as_nanos() as u64;
        if due.publication && registry.publish_at(now) {
            registry.revision += 1;
        }
        let mut pending_motions = 0;
        for ((id, input, _), receiver) in inputs.iter().zip(&mut receivers) {
            receiver.self_update(std::mem::take(&mut *input.update.borrow_mut()));
            receiver.observer.reconcile(&registry, *id, &receiver.terrain).unwrap();
            receiver.world_observer.reconcile(&registry.arena, &receiver.terrain);
            queue_peak = queue_peak.max(receiver.observer.queued_frames());
        }
        for _ in 0..WRITER_ROUNDS {
            let mut pending = false;
            for receiver in &mut receivers {
                if receiver.observer.queued_frames() > 0
                    || receiver.observer.delivery_lag(&registry).0 > 0
                    || receiver.world_observer.pending_work()
                {
                    receiver.drain(&registry);
                    pending = true;
                }
            }
            if !pending {
                break;
            }
        }
        for ((id, input, _), receiver) in inputs.iter().zip(&mut receivers) {
            let (pending, lag) = receiver.observer.delivery_lag(&registry);
            pending_motions += pending;
            max_revision_lag = max_revision_lag.max(lag);
            // Memory-sink acknowledgements model the normal confirmation boundary.
            if let Some(teleport) = registry.players[id].state.life.pending_teleport() {
                input.teleport_confirmation.set(Some(teleport.id));
            }
        }
        peak_pending_motions = peak_pending_motions.max(pending_motions);
        let cpu_ns = start.elapsed().as_nanos() as u64;
        if tick >= WARMUP {
            combined.push(cpu_ns);
            simulation.push(sim_ns);
            lateness.push(due.lateness.as_nanos() as u64);
        }
        if paced {
            glommio::executor().yield_now().await;
        }
    }
    let before_drain: usize =
        receivers.iter().map(|receiver| receiver.observer.queued_frames()).sum();
    for _ in 0..20 {
        if registry.publish_at(clock.next_deadline()) {
            registry.revision += 1;
        }
        for ((id, _, _), receiver) in inputs.iter().zip(&mut receivers) {
            receiver.observer.reconcile(&registry, *id, &receiver.terrain).unwrap();
            receiver.drain(&registry);
        }
    }
    let remaining: usize = receivers.iter().map(|receiver| receiver.observer.queued_frames()).sum();
    assert_eq!(remaining, 0, "bounded sinks must converge after input stops");
    assert!(receivers.iter().all(|receiver| receiver.observer.delivery_lag(&registry) == (0, 0)));
    assert_eq!(corrections, 0, "valid fixture movement should not need corrections");
    let stats = |values: &mut Vec<u64>| {
        values.sort_unstable();
        json!({
            "p95_us": values[values.len() * 95 / 100] as f64 / 1000.0,
            "p99_us": values[values.len() * 99 / 100] as f64 / 1000.0,
            "max_us": values[values.len() - 1] as f64 / 1000.0,
        })
    };
    let over_budget = combined.iter().filter(|&&ns| u128::from(ns) > DEADLINE_NS).count();
    let deadline_misses = combined
        .iter()
        .zip(&lateness)
        .filter(|(cpu, late)| u128::from(**cpu) + u128::from(**late) > DEADLINE_NS)
        .count();
    if case.medium == Medium::Water {
        assert!(swimming_steps >= samples * PLAYERS / 2);
    }
    if case.medium == Medium::Crawl {
        assert!(crawling_steps >= samples * PLAYERS / 2);
    }
    let result = json!({
        "scenario": case.name,
        "players": PLAYERS,
        "receivers": PLAYERS,
        "input_period": case.input_period,
        "paced": paced,
        "combined": stats(&mut combined),
        "simulation": stats(&mut simulation),
        "lateness": stats(&mut lateness),
        "over_budget": over_budget,
        "deadline_misses": deadline_misses,
        "samples": samples,
        "skipped_steps": skipped,
        "corrections": corrections,
        "collision_cells": cells,
        "swimming_player_steps": swimming_steps,
        "crawling_player_steps": crawling_steps,
        "queue_peak_frames": queue_peak,
        "peak_pending_motions": peak_pending_motions,
        "max_revision_lag": max_revision_lag,
        "pending_frames_before_final_drain": before_drain,
        "pending_frames_after_final_drain": remaining,
        "terrain_bytes": world.terrain_bytes,
        "registry_payload_bytes": registry.capacity_payload_bytes(),
        "shared_motion_payload_bytes": crate::movement::shared_motion_bytes(),
        "shared_terrain_catalog_bytes": halcyon_world::collision_catalog_bytes(),
        "world_observer_payload_bytes": receivers.iter().map(|r|r.world_observer.capacity_payload_bytes()).sum::<usize>(),
        "sent_bytes": receivers.iter().map(|receiver| receiver.stream.inner.bytes).sum::<u64>(),
    });
    println!("{}", result);
    result
}

fn report(cases: Vec<Value>, paced: bool) {
    let report = json!({
        "schema_version": 2,
        "simulation_hz": crate::simulation::SIMULATION_HZ,
        "publication_hz": 20,
        "writer_round_limit": WRITER_ROUNDS,
        "encrypted": true,
        "paced": paced,
        "excludes": ["sockets", "authentication", "chunk streaming", "unsupported movement media"],
        "cases": cases,
    });
    if let Some(path) = std::env::var_os("HALCYON_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    if let Ok(limit) = std::env::var("HALCYON_BENCH_MAX_P99_US") {
        let limit: f64 = limit.parse().unwrap();
        assert!(limit.is_finite() && limit > 0.0);
        for case in report["cases"].as_array().unwrap() {
            assert!(
                case["combined"]["p99_us"].as_f64().unwrap() <= limit,
                "combined p99 exceeds {limit} us"
            );
        }
    }
}

#[test]
#[ignore = "release benchmark of terrain, gameplay, publication and 100 encrypted sinks"]
fn gameplay_world_benchmark() {
    block_on(async {
        let mut cases = Vec::new();
        for case in [
            Scenario {
                name: "crowded_water_swimming",
                spread: false,
                input_period: 1,
                steps: false,
                churn: false,
                driven: true,
                falling: false,
                medium: Medium::Water,
            },
            Scenario {
                name: "crowded_dry_crawling",
                spread: false,
                input_period: 1,
                steps: false,
                churn: false,
                driven: true,
                falling: false,
                medium: Medium::Crawl,
            },
            Scenario {
                name: "idle",
                spread: false,
                input_period: 0,
                steps: false,
                churn: false,
                driven: false,
                falling: false,
                medium: Medium::Dry,
            },
            Scenario {
                name: "spread_20hz",
                spread: true,
                input_period: 1,
                steps: false,
                churn: false,
                driven: false,
                falling: false,
                medium: Medium::Dry,
            },
            Scenario {
                name: "crowded_20hz",
                spread: false,
                input_period: 1,
                steps: false,
                churn: false,
                driven: false,
                falling: false,
                medium: Medium::Dry,
            },
            Scenario {
                name: "crowded_ground_surfaces",
                spread: false,
                input_period: 1,
                steps: false,
                churn: false,
                driven: true,
                falling: false,
                medium: Medium::Dry,
            },
            Scenario {
                name: "steps_equipment_lifecycle",
                spread: false,
                input_period: 1,
                steps: true,
                churn: true,
                driven: false,
                falling: false,
                medium: Medium::Dry,
            },
            Scenario {
                name: "crowded_falling_blocks",
                spread: false,
                input_period: 1,
                steps: false,
                churn: false,
                driven: false,
                falling: true,
                medium: Medium::Dry,
            },
        ] {
            cases.push(measure(case, false).await);
        }
        report(cases, false);
    });
}

#[test]
#[ignore = "wall-clock Glommio measurement; requires io_uring and approximately 11 seconds"]
fn gameplay_scheduler_benchmark() {
    glommio::LocalExecutorBuilder::new(glommio::Placement::Unbound).make().unwrap().run(async {
        let result = measure(
            Scenario {
                name: "paced_ground_falling_lifecycle",
                spread: false,
                input_period: 1,
                steps: true,
                churn: true,
                driven: true,
                falling: true,
                medium: Medium::Dry,
            },
            true,
        )
        .await;
        report(vec![result], true);
    });
}
