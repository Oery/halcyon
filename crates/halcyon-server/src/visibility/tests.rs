use super::*;
use crate::lobby::Profile;
use mc_protocol::packets::PacketWriter;
use mc_protocol::packets::client::PlayerInfoUpdate;
use mc_protocol::types::VarInt;

pub(super) fn player(id: i32, x: f64, revision: u64) -> PlayerSnapshot {
    let username = format!("player{id}");
    let uuid = Uuid::from_u128(id as u128 + 1);
    let info =
        encode(&PlayerInfoUpdate { uuid, username: &username, properties: Vec::new() }).unwrap();
    PlayerSnapshot {
        entity_id: id,
        profile: Rc::new(Profile { uuid, username, info }),
        transform: Transform { x, y: 64.0, z: 1.0, ..Transform::default() },
        skin_parts: 0x7f,
        sneaking: false,
        crouching: false,
        sprinting: false,
        health: 20.0,
        equipment: Rc::new(std::array::from_fn(|_| mc_protocol::types::ItemStack::default())),
        life_generation: 0,
        visible: true,
        revision,
    }
}

fn packets(mut bytes: &[u8]) -> Vec<(i32, Vec<u8>)> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let length = VarInt::read(&mut bytes).unwrap().0 as usize;
        let mut payload = &bytes[..length];
        let id = VarInt::read(&mut payload).unwrap().0;
        out.push((id, payload.to_vec()));
        bytes = &bytes[length..];
    }
    out
}

fn drain(observer: &mut Observer) -> Vec<(i32, Vec<u8>)> {
    let mut out = Vec::new();
    while let Some(bytes) = observer.next_frame().unwrap() {
        out.extend(packets(&bytes));
        observer.frame_sent();
    }
    assert_eq!(observer.queued_bytes(), 0);
    assert_eq!(observer.queued_frames(), 0);
    out
}

#[test]
fn tab_membership_terrain_visibility_and_unload_order() {
    let local = player(0, 1.0, 1);
    let near = player(1, 2.0, 1);
    let far = player(2, 160.0, 1);
    let registry = Registry::from_snapshots(vec![local.clone(), near.clone(), far.clone()], 1);
    let terrain = HashSet::from([(0, 0)]);
    let mut observer = Observer::default();
    observer.reconcile(&registry, 0, &terrain).unwrap();
    let delivered = drain(&mut observer);
    let profile_index =
        delivered.iter().rposition(|(id, _)| *id == PlayerInfoUpdate::ID.0).unwrap();
    let spawn_index = delivered.iter().position(|(id, _)| *id == SpawnPlayer::ID.0).unwrap();
    assert!(profile_index < spawn_index);
    assert_eq!(observer.roster_count(), 3);
    assert_eq!(observer.visible_count(), 1);

    observer.before_unload((0, 0)).unwrap();
    assert!(observer.has_unload_barrier());
    let removal = observer.next_frame().unwrap().unwrap();
    assert_eq!(packets(&removal)[0].0, RemoveEntities::ID.0);
    observer.frame_sent();
    assert!(!observer.has_unload_barrier());
    assert_eq!(observer.visible_count(), 0);
    assert_eq!(observer.roster_count(), 3);

    let departed = Registry::from_snapshots(vec![local, far], 2);
    observer.reconcile(&departed, 0, &HashSet::new()).unwrap();
    let delivered = drain(&mut observer);
    assert_eq!(delivered.iter().filter(|(id, _)| *id == PlayerInfoRemove::ID.0).count(), 1);
    assert_eq!(observer.roster_count(), 2);
}

#[test]
fn equipment_uses_delivered_baselines_and_respawn_recreates_the_entity() {
    use mc_protocol::types::ItemStack;
    let local = player(0, 1.0, 1);
    let mut near = player(1, 2.0, 1);
    near.equipment = Rc::new(std::array::from_fn(|slot| {
        if slot == 0 {
            ItemStack::plain(1, 1).unwrap()
        } else {
            ItemStack::default()
        }
    }));
    let terrain = HashSet::from([(0, 0)]);
    let mut observer = Observer::default();
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 1), 0, &terrain)
        .unwrap();
    assert!(drain(&mut observer).iter().any(|(id, _)| *id == SetEquipment::ID.0));
    near.equipment = Rc::new(std::array::from_fn(|slot| {
        if slot == 0 {
            ItemStack::plain(2, 1).unwrap()
        } else {
            ItemStack::default()
        }
    }));
    near.revision = 2;
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 2), 0, &terrain)
        .unwrap();
    let selected = observer.next_frame().unwrap().unwrap();
    near.equipment = Rc::new(std::array::from_fn(|slot| {
        if slot == 0 {
            ItemStack::plain(3, 1).unwrap()
        } else {
            ItemStack::default()
        }
    }));
    near.revision = 3;
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 3), 0, &terrain)
        .unwrap();
    assert_eq!(observer.next_frame().unwrap().unwrap(), selected);
    observer.frame_sent();
    assert!(drain(&mut observer).iter().any(|(id, _)| *id == SetEquipment::ID.0));
    assert_eq!(observer.rendered[&1].equipment, near.equipment);
    near.life_generation = 1;
    near.revision = 4;
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 4), 0, &terrain)
        .unwrap();
    assert!(drain(&mut observer).iter().any(|(id, _)| *id == RemoveEntities::ID.0));
    observer.reconcile(&Registry::from_snapshots(vec![local, near], 5), 0, &terrain).unwrap();
    let packets = drain(&mut observer);
    assert!(packets.iter().any(|(id, _)| *id == SpawnPlayer::ID.0));
    assert!(!packets.iter().any(|(id, _)| *id == PlayerInfoRemove::ID.0));
    assert_eq!(observer.roster_count(), 2);
}

#[test]
fn movement_coalesces_against_only_successfully_delivered_baseline() {
    let local = player(0, 1.0, 1);
    let terrain = HashSet::from([(0, 0), (2, 0)]);
    let mut observer = Observer::default();
    observer
        .reconcile(
            &Registry::from_snapshots(vec![local.clone(), player(1, 1.0, 1)], 1),
            0,
            &terrain,
        )
        .unwrap();
    drain(&mut observer);
    for revision in [2, 3] {
        observer
            .reconcile(
                &Registry::from_snapshots(
                    vec![local.clone(), player(1, revision as f64, revision)],
                    revision,
                ),
                0,
                &terrain,
            )
            .unwrap();
    }
    assert_eq!(observer.coalesced_updates(), 1);
    let bytes = observer.next_frame().unwrap().unwrap();
    assert!(Rc::ptr_eq(&bytes, &observer.next_frame().unwrap().unwrap()));
    assert_eq!(observer.rendered[&1].transform.x, 1.0);
    let encoded = packets(&bytes);
    assert_eq!(encoded[0].0, MoveEntityPosition::ID.0);
    let movement = MoveEntityPosition::decode_payload(&mut encoded[0].1.as_slice()).unwrap();
    assert_eq!(movement.dx, 2 * 4096);
    observer.frame_sent();
    assert_eq!(observer.rendered[&1].transform.x, 3.0);

    observer
        .reconcile(&Registry::from_snapshots(vec![local, player(1, 40.0, 4)], 4), 0, &terrain)
        .unwrap();
    assert_eq!(packets(&observer.next_frame().unwrap().unwrap())[0].0, EntityPositionSync::ID.0);
    observer.frame_sent();
    assert_eq!(observer.rendered[&1].transform.x, 40.0);

    // A writer budget can select a spawn but defer its write to a later
    // pass. New publications must survive that old spawn's commit.
    let mut staged = Observer::default();
    staged
        .reconcile(
            &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 1.0, 1)], 1),
            0,
            &terrain,
        )
        .unwrap();
    loop {
        staged.next_frame().unwrap().unwrap();
        if matches!(staged.active.as_ref().unwrap().delivery, Delivery::Spawn(_)) {
            break;
        }
        staged.frame_sent();
    }
    staged
        .reconcile(
            &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 2.0, 2)], 2),
            0,
            &terrain,
        )
        .unwrap();
    staged.frame_sent();
    let pending = packets(&staged.next_frame().unwrap().unwrap());
    let movement = MoveEntityPosition::decode_payload(&mut pending[0].1.as_slice()).unwrap();
    assert_eq!(movement.dx, 4096);
    staged.frame_sent();
    assert_eq!(staged.rendered[&1].transform.x, 2.0);
}

#[test]
fn relative_positions_follow_java_rounding_at_positive_and_negative_half_units() {
    for (old, next, delta) in [(-0.5, 0.5, 1), (-0.51, -0.49, 1), (0.49, 0.51, 1), (0.51, 0.49, -1)]
    {
        let previous = player(1, old / 4096.0, 1);
        let current = player(1, next / 4096.0, 2);
        let frame = movement_frame(current, &rendered(&previous)).unwrap().unwrap();
        let encoded = packets(&frame.bytes);
        let movement = MoveEntityPosition::decode_payload(&mut encoded[0].1.as_slice()).unwrap();
        assert_eq!(movement.dx, delta, "old={old} next={next}");
    }
}

#[test]
fn unpublished_spawn_and_selected_movement_are_cancelled_before_unload() {
    let local = player(0, 1.0, 1);
    let terrain = HashSet::from([(0, 0)]);
    let registry = Registry::from_snapshots(vec![local.clone(), player(1, 2.0, 1)], 1);
    let mut observer = Observer::default();
    observer.reconcile(&registry, 0, &terrain).unwrap();
    observer.before_unload((0, 0)).unwrap();
    assert!(drain(&mut observer).iter().all(|(id, _)| *id != SpawnPlayer::ID.0));
    assert_eq!(observer.visible_count(), 0);

    observer
        .reconcile(
            &Registry::from_snapshots(vec![local.clone(), player(1, 2.0, 2)], 2),
            0,
            &terrain,
        )
        .unwrap();
    drain(&mut observer);
    observer
        .reconcile(&Registry::from_snapshots(vec![local, player(1, 3.0, 3)], 3), 0, &terrain)
        .unwrap();
    assert!(observer.next_frame().unwrap().is_some());
    observer.before_unload((0, 0)).unwrap();
    let delivered = drain(&mut observer);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, RemoveEntities::ID.0);

    let mut selected = Observer::default();
    selected
        .reconcile(
            &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 2.0, 1)], 1),
            0,
            &terrain,
        )
        .unwrap();
    loop {
        selected.next_frame().unwrap().unwrap();
        if matches!(selected.active.as_ref().unwrap().delivery, Delivery::Spawn(_)) {
            break;
        }
        selected.frame_sent();
    }
    selected.reconcile(&Registry::from_snapshots(vec![player(0, 1.0, 1)], 2), 0, &terrain).unwrap();
    assert!(drain(&mut selected).iter().all(|(id, _)| *id != SpawnPlayer::ID.0));
    assert_eq!(selected.visible_count(), 0);
    assert_eq!(selected.roster_count(), 1);
}

#[test]
fn admissions_are_paced_per_publication_and_backlog_is_bounded() {
    let records: Vec<_> = (0..40).map(|id| player(id, 1.0, 1)).collect();
    let registry = Registry::from_snapshots(records.clone(), 1);
    let mut observer = Observer::default();
    observer.reconcile(&registry, 0, &HashSet::new()).unwrap();
    drain(&mut observer);
    observer.reconcile(&registry, 0, &HashSet::new()).unwrap();
    assert!(drain(&mut observer).is_empty());
    assert_eq!(observer.roster_count(), 16);
    observer.reconcile(&Registry::from_snapshots(records, 2), 0, &HashSet::new()).unwrap();
    drain(&mut observer);
    assert_eq!(observer.roster_count(), 32);

    let mut observer = Observer::default();
    assert!(matches!(observer.reserve(MAX_BYTES + 1, 1), Err(VisibilityError::Overflow)));
    assert_eq!(observer.queued_bytes(), 0);
    observer.reserve(MAX_BYTES, MAX_FRAMES).unwrap();
    assert!(matches!(observer.reserve(0, 1), Err(VisibilityError::Overflow)));
    assert!(matches!(observer.reserve(1, 0), Err(VisibilityError::Overflow)));
    assert_eq!(observer.queued_bytes(), MAX_BYTES);
    assert_eq!(observer.queued_frames(), MAX_FRAMES);
}

#[test]
fn continuously_moving_players_share_a_limited_writer_budget() {
    let players: Vec<_> = (0..80).map(|id| player(id, 1.0, 1)).collect();
    let mut registry = Registry::from_snapshots(players.clone(), 1);
    let terrain = HashSet::from([(0, 0)]);
    let mut observer = Observer::default();
    for _ in 0..6 {
        registry.publish_transforms([]);
        observer.reconcile(&registry, 0, &terrain).unwrap();
        drain(&mut observer);
    }
    assert_eq!(observer.visible_count(), 79);
    for tick in 0..3 {
        registry.publish_transforms(players.iter().map(|player| {
            let mut transform = player.transform;
            transform.x += 0.25 + tick as f64 * 0.01;
            (player.entity_id, transform)
        }));
        observer.reconcile(&registry, 0, &terrain).unwrap();
        let mut frames = 0;
        while frames < 32 && observer.next_frame().unwrap().is_some() {
            frames += observer.next_frame_count();
            observer.frame_sent();
        }
        assert_eq!(observer.motion_waiting.len(), observer.latest.len());
    }
    assert!(observer.rendered.values().all(|player| player.transform.x > 1.0));
}

#[test]
#[ignore = "manual synthetic lobby fanout measurement; no sockets or gameplay actors"]
fn synthetic_lobby() {
    use std::time::Instant;

    fn drain_sink(observer: &mut Observer) -> (usize, usize) {
        let mut bytes = 0;
        let mut frames = 0;
        while let Some(frame) = observer.next_frame().unwrap() {
            bytes += frame.len();
            frames += observer.next_frame_count();
            observer.frame_sent();
        }
        (bytes, frames)
    }

    for population in [128, 1024, 4096] {
        for dense in [false, true] {
            let players: Vec<_> = (0..population)
                .map(|id| player(id, if dense { 1.0 } else { id as f64 * 32.0 + 1.0 }, 1))
                .collect();
            let profile_payload: usize = players
                .iter()
                .map(|player| {
                    std::mem::size_of::<Profile>()
                        + player.profile.username.len()
                        + player.profile.info.len()
                })
                .sum();
            let mut registry = Registry::from_snapshots(players.clone(), 1);
            let mut observers: Vec<_> = (0..16)
                .map(|id| {
                    let terrain = if dense {
                        HashSet::from([(0, 0)])
                    } else {
                        HashSet::from([(id * 2, 0), ((id + 1) * 2, 0)])
                    };
                    (Observer::default(), terrain)
                })
                .collect();

            // Complete the same paced admissions as production, outside
            // the measured window. No full-roster burst is manufactured.
            let admission_start = Instant::now();
            let mut admission_bytes = 0;
            for _ in 0..(population as usize).div_ceil(16) + 2 {
                registry.publish_transforms([]);
                for (id, (observer, terrain)) in observers.iter_mut().enumerate() {
                    observer.reconcile(&registry, id as i32, terrain).unwrap();
                    admission_bytes += drain_sink(observer).0;
                }
            }
            let admission_us = admission_start.elapsed().as_micros();
            assert!(
                observers
                    .iter()
                    .all(|(observer, _)| observer.roster_count() == population as usize)
            );
            let edges: usize = observers.iter().map(|(observer, _)| observer.visible_count()).sum();
            assert!(edges <= 16 * 4096);
            let mut samples = [0u64; 1000];
            let mut bytes = 0usize;
            let mut frames = 0usize;
            let mut coalesced_start = 0;
            for tick in 0..1100 {
                if tick == 100 {
                    coalesced_start = observers
                        .iter()
                        .map(|(observer, _)| observer.coalesced_updates())
                        .sum::<u64>();
                }
                let start = Instant::now();
                let offset = if tick % 2 == 0 { 0.25 } else { 0.5 };
                registry.publish_transforms(players.iter().map(|player| {
                    let mut transform = player.transform;
                    transform.x += offset;
                    transform.yaw = (tick % 360) as f32;
                    (player.entity_id, transform)
                }));
                for (id, (observer, terrain)) in observers.iter_mut().enumerate() {
                    observer.reconcile(&registry, id as i32, terrain).unwrap();
                    // Withhold one drain in four to exercise replacement
                    // of pending movement before it is encoded/delivered.
                    if tick % 4 != 0 {
                        let (sent_bytes, sent_frames) = drain_sink(observer);
                        if tick >= 100 {
                            bytes += sent_bytes;
                            frames += sent_frames;
                        }
                    }
                }
                if tick >= 100 {
                    samples[tick - 100] = start.elapsed().as_nanos() as u64;
                }
            }
            samples.sort_unstable();
            let mean_us = samples.iter().map(|sample| *sample as f64).sum::<f64>() / 1_000_000.0;
            let coalesced =
                observers.iter().map(|(observer, _)| observer.coalesced_updates()).sum::<u64>()
                    - coalesced_start;
            let high_water =
                observers.iter().map(|(observer, _)| observer.high_water_bytes()).max().unwrap();
            let delivered_bytes: u64 =
                observers.iter().map(|(observer, _)| observer.sent_bytes()).sum();
            let observer_capacity_payload_bytes: usize =
                observers.iter().map(|(observer, _)| observer.capacity_payload_bytes()).sum();
            let shared_registry_capacity_payload_bytes = registry.capacity_payload_bytes();

            // A sink that never drains must either retain a bounded set
            // of coalesced state or reject further reliable admissions.
            let mut stalled = Observer::default();
            let mut overflow = false;
            for tick in 0..130 {
                let mut transform = players[1].transform;
                transform.x += if tick % 2 == 0 { 0.125 } else { 0.375 };
                registry.publish_transforms([(1, transform)]);
                let (healthy, terrain) = &mut observers[0];
                healthy.reconcile(&registry, 0, terrain).unwrap();
                assert!(drain_sink(healthy).1 > 0);
                match stalled.reconcile(&registry, 0, &observers[0].1) {
                    Ok(()) => {}
                    Err(VisibilityError::Overflow) => {
                        overflow = true;
                        break;
                    }
                    Err(error) => panic!("unexpected encoding error: {error}"),
                }
                assert!(stalled.queued_bytes() <= MAX_BYTES);
                assert!(stalled.queued_frames() <= MAX_FRAMES);
            }
            assert_eq!(registry.active_count(), population as usize);
            if population == 4096 {
                assert!(overflow);
            }
            eprintln!(
                "synthetic lobby population={population} layout={} observers=16 visible_edges={edges} admission_us={admission_us} admission_encoded_bytes={admission_bytes} ticks=1000 mean_us={mean_us:.1} p95_us={:.1} max_us={:.1} encoded_bytes={bytes} protocol_frames={frames} coalesced={coalesced} high_water_bytes={high_water} delivered_bytes_including_warmup={delivered_bytes} shared_profile_payload_bytes_per_actor={:.1} shared_registry_capacity_payload_bytes={shared_registry_capacity_payload_bytes} observer_capacity_payload_bytes={observer_capacity_payload_bytes} stalled_overflow={overflow} stalled_bytes={} stalled_frames={} (in-memory sinks; excludes sockets, encryption, chunks, shared scheduler, hash controls, Rc headers and allocator overhead)",
                if dense { "dense" } else { "sparse" },
                samples[949] as f64 / 1000.0,
                samples[999] as f64 / 1000.0,
                profile_payload as f64 / population as f64,
                stalled.queued_bytes(),
                stalled.queued_frames(),
            );
        }
    }
}
