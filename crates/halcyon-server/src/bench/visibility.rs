//! Opt-in visibility component benchmarks. No sockets, accounts, bots, or running server.

use super::*;
use crate::encryption::ciphers;
use crate::transport::ClientStream;
use futures_lite::{AsyncWrite, AsyncWriteExt};
use serde_json::{Value, json};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

const PLAYERS: i32 = 100;
const WORKLOAD_HZ: u64 = 120;
const DEADLINE_NS: u64 = 1_000_000_000 / WORKLOAD_HZ;
const WARMUP: usize = 120;
const SAMPLES: usize = 1_200;

#[derive(Default)]
struct CountingWriter {
    bytes: u64,
}

impl AsyncWrite for CountingWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes += bytes.len() as u64;
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct Receiver {
    observer: Observer,
    terrain: HashSet<Column>,
    stream: ClientStream<CountingWriter>,
}

impl Receiver {
    fn new(id: i32, sparse: bool) -> Self {
        let (enc, dec) = ciphers(&[7; 16]).unwrap();
        let mut stream = ClientStream::new(CountingWriter::default());
        stream.enable_encryption(enc, dec).unwrap();
        let center = if sparse { id * 4 } else { 0 };
        let terrain = (-8..=8).flat_map(|dx| (-8..=8).map(move |dz| (center + dx, dz))).collect();
        Self { observer: Observer::default(), terrain, stream }
    }

    /// One production-sized writer pass. A packet group remains indivisible.
    fn drain_slice(&mut self) {
        block_on(async {
            let mut frames = 0;
            let mut bytes = 0;
            while frames < WRITE_SLICE_FRAMES && bytes < WRITE_SLICE_BYTES {
                let Some(frame) = self.observer.next_frame().unwrap() else {
                    break;
                };
                frames += self.observer.next_frame_count();
                bytes += frame.len();
                self.stream.write_all(&frame).await.unwrap();
                self.stream.flush().await.unwrap();
                self.observer.frame_sent();
            }
        });
    }
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    sparse: bool,
    input_period: usize,
    visibility_churn: bool,
}

fn measure(scenario: Scenario) -> Value {
    let players: Vec<_> = (0..PLAYERS)
        .map(|id| tests::player(id, if scenario.sparse { id as f64 * 64.0 + 1.0 } else { 1.0 }, 1))
        .collect();
    let mut registry = Registry::from_snapshots(players.clone(), 1);
    let mut receivers: Vec<_> = (0..PLAYERS).map(|id| Receiver::new(id, scenario.sparse)).collect();

    // Roster and entity admission use the same pacing and drain budgets as runtime.
    // Setup is checked for completion and excluded from steady-state timings.
    let setup = Instant::now();
    for _ in 0..120 {
        registry.publish_transforms([]);
        for (id, receiver) in receivers.iter_mut().enumerate() {
            receiver.observer.reconcile(&registry, id as i32, &receiver.terrain).unwrap();
            receiver.drain_slice();
        }
        if receivers.iter().enumerate().all(|(id, receiver)| {
            let expected = registry.candidates(&receiver.terrain).len() - 1;
            receiver.observer.roster_count() == PLAYERS as usize
                && receiver.observer.visible_count() == expected
                && receiver.observer.queued_frames() == 0
                && receiver.observer.latest.is_empty()
                && registry.get(id as i32).is_some()
        }) {
            break;
        }
    }
    assert!(
        receivers.iter().all(|receiver| {
            receiver.observer.roster_count() == PLAYERS as usize
                && receiver.observer.visible_count()
                    == registry.candidates(&receiver.terrain).len() - 1
                && receiver.observer.queued_frames() == 0
        }),
        "paced setup did not complete"
    );
    let setup_us = setup.elapsed().as_micros() as u64;

    let mut samples = [0u64; SAMPLES];
    let mut sent_before = 0;
    let mut queue_peak_bytes = 0;
    let mut queue_peak_frames = 0;
    let empty = HashSet::new();
    for tick in 0..WARMUP + SAMPLES {
        if tick == WARMUP {
            sent_before = receivers.iter().map(|receiver| receiver.stream.inner.bytes).sum::<u64>();
        }
        let start = Instant::now();
        if scenario.input_period != 0 && tick % scenario.input_period == 0 {
            let update = tick / scenario.input_period;
            registry.publish_transforms(players.iter().map(|player| {
                let mut transform = player.transform;
                transform.x += if update.is_multiple_of(2) { 0.25 } else { 0.5 };
                transform.yaw = (update % 360) as f32;
                (player.entity_id, transform)
            }));
        } else {
            registry.publish_transforms([]);
        }
        for (id, receiver) in receivers.iter_mut().enumerate() {
            // Alternate half the observers between loaded terrain and no terrain
            // every half simulated second, exercising spawn/removal churn.
            let unavailable = scenario.visibility_churn && id % 2 == 0 && (tick / 60) % 2 == 1;
            let terrain = if unavailable { &empty } else { &receiver.terrain };
            receiver.observer.reconcile(&registry, id as i32, terrain).unwrap();
            if tick >= WARMUP {
                queue_peak_bytes = queue_peak_bytes.max(receiver.observer.queued_bytes());
                queue_peak_frames = queue_peak_frames.max(receiver.observer.queued_frames());
            }
            receiver.drain_slice();
        }
        if tick >= WARMUP {
            samples[tick - WARMUP] = start.elapsed().as_nanos() as u64;
        }
    }
    let bytes =
        receivers.iter().map(|receiver| receiver.stream.inner.bytes).sum::<u64>() - sent_before;
    let pending_motions: usize =
        receivers.iter().map(|receiver| receiver.observer.latest.len()).sum();
    let max_revision_lag = receivers
        .iter()
        .flat_map(|receiver| receiver.observer.rendered.iter())
        .filter_map(|(id, rendered)| {
            registry.get(*id).map(|current| current.revision - rendered.revision)
        })
        .max()
        .unwrap_or(0);
    let observer_payload: usize =
        receivers.iter().map(|receiver| receiver.observer.capacity_payload_bytes()).sum();
    samples.sort_unstable();
    let mean_us =
        samples.iter().map(|sample| *sample as f64).sum::<f64>() / SAMPLES as f64 / 1000.0;
    let p95_us = samples[SAMPLES * 95 / 100 - 1] as f64 / 1000.0;
    let p99_us = samples[SAMPLES * 99 / 100 - 1] as f64 / 1000.0;
    let over_budget = samples.iter().filter(|sample| **sample > DEADLINE_NS).count();
    eprintln!(
        "{}: players=100 receivers=100 mean_us={mean_us:.1} p95_us={p95_us:.1} p99_us={p99_us:.1} cpu_over_budget={over_budget}/{SAMPLES} final_pending_motions={pending_motions} final_max_revision_lag={max_revision_lag}",
        scenario.name,
    );
    json!({
        "scenario": scenario.name, "players": PLAYERS, "receivers": PLAYERS,
        "input_hz": if scenario.input_period == 0 { 0 } else { WORKLOAD_HZ / scenario.input_period as u64 },
        "setup_us": setup_us, "samples": SAMPLES, "mean_us": mean_us,
        "p95_us": p95_us, "p99_us": p99_us, "max_us": samples[SAMPLES - 1] as f64 / 1000.0,
        "cpu_over_budget_samples": over_budget, "encrypted_sink_bytes": bytes,
        "queue_peak_bytes_per_receiver": queue_peak_bytes,
        "queue_peak_frames_per_receiver": queue_peak_frames,
        "final_visible_edges": receivers.iter().map(|receiver| receiver.observer.visible_count()).sum::<usize>(),
        "final_pending_motions": pending_motions, "final_max_revision_lag": max_revision_lag,
        "registry_payload_capacity_bytes": registry.capacity_payload_bytes(),
        "observer_payload_capacity_bytes": observer_payload,
    })
}

#[test]
#[ignore = "opt-in release benchmark of a 100-player world with 100 encrypted memory sinks"]
// An accidental debug run must fail loudly instead of recording a misleading baseline.
#[allow(clippy::assertions_on_constants)]
fn target_world_benchmark() {
    assert!(!cfg!(debug_assertions), "run this benchmark with cargo test --release");
    let cases = [
        Scenario { name: "idle", sparse: false, input_period: 0, visibility_churn: false },
        Scenario {
            name: "spread_20hz_input",
            sparse: true,
            input_period: 6,
            visibility_churn: false,
        },
        Scenario {
            name: "crowded_20hz_input",
            sparse: false,
            input_period: 6,
            visibility_churn: false,
        },
        Scenario {
            name: "crowded_120hz_input",
            sparse: false,
            input_period: 1,
            visibility_churn: false,
        },
        Scenario {
            name: "visibility_churn",
            sparse: false,
            input_period: 6,
            visibility_churn: true,
        },
    ]
    .map(measure);
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|info| {
        info.lines()
            .find_map(|line| {
                line.strip_prefix("model name").and_then(|value| value.split_once(':'))
            })
            .map(|(_, model)| model.trim().to_owned())
    });
    let report = json!({
        "schema_version": 1, "workload_hz": WORKLOAD_HZ, "cpu_budget_ns": DEADLINE_NS,
        "writer_slice_frames": WRITE_SLICE_FRAMES, "writer_slice_bytes": WRITE_SLICE_BYTES,
        "warmup_steps": WARMUP, "architecture": std::env::consts::ARCH,
        "cpu_model": cpu_model, "debug_assertions": cfg!(debug_assertions),
        "encrypted": true, "paced_wall_clock": false,
        "excludes": ["sockets", "authentication", "chunks", "executor scheduling", "gameplay physics"],
        "memory_estimate_excludes": ["hash controls", "Rc headers", "allocator overhead", "transport buffers"],
        "cases": cases,
    });
    if let Some(path) = std::env::var_os("HALCYON_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    if let Ok(limit) = std::env::var("HALCYON_BENCH_MAX_P99_US") {
        let limit: f64 = limit.parse().expect("HALCYON_BENCH_MAX_P99_US must be a positive number");
        assert!(limit.is_finite() && limit > 0.0, "p99 limit must be finite and positive");
        for case in report["cases"].as_array().unwrap() {
            assert!(
                case["p99_us"].as_f64().unwrap() <= limit,
                "{} exceeds configured p99 CPU budget of {limit} us",
                case["scenario"]
            );
        }
    }
}
