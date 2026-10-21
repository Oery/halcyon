//! Larger synthetic lobby measurement with unencrypted memory sinks.

use super::tests::player;
use super::*;
use crate::lobby::Profile;

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
