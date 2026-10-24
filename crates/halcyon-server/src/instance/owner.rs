//! Instance scheduling, registration handling and aggregate diagnostics.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glommio::channels::local_channel;
use glommio::timer::Timer;

use super::session::{Input, Signal};
use super::{
    GameCtx, Metrics, ObserverMetrics, REGISTRATIONS_PER_SLICE, Registration, Registry, Status,
};
use crate::game::Game;
use crate::simulation::SimulationClock;

/// The instance's game, driven by its owner task.
pub(super) struct Driver {
    game: Box<dyn Game>,
    status: Rc<Status>,
    map: Arc<crate::map::Map>,
    close: bool,
}

impl Driver {
    pub(super) fn new(game: Box<dyn Game>, status: Rc<Status>, map: Arc<crate::map::Map>) -> Self {
        Self { game, status, map, close: false }
    }

    /// Runs the game's `on_create` before any player can be admitted.
    pub(super) fn create(&mut self, registry: &mut Registry) {
        let mut notices = Vec::new();
        let mut ctx = GameCtx {
            id: self.status.id,
            registry,
            world: &self.map.world,
            notices: &mut notices,
            close: &mut self.close,
        };
        self.game.on_create(&mut ctx);
    }

    fn join(&mut self, registry: &mut Registry, id: i32, notices: &mut Vec<Rc<Input>>) {
        let mut ctx = GameCtx {
            id: self.status.id,
            registry,
            world: &self.map.world,
            notices,
            close: &mut self.close,
        };
        self.game.on_join(&mut ctx, id);
    }

    /// Events, then the game's own step; returns whether the instance is done.
    fn step(&mut self, registry: &mut Registry, notices: &mut Vec<Rc<Input>>) -> bool {
        let mut events = std::mem::take(&mut registry.events);
        let mut ctx = GameCtx {
            id: self.status.id,
            registry,
            world: &self.map.world,
            notices,
            close: &mut self.close,
        };
        for event in events.drain(..) {
            self.game.on_event(&mut ctx, event);
        }
        self.game.tick(&mut ctx);
        // Keep the event capacity for the next step.
        registry.events = events;
        self.status.phase.set(self.game.phase());
        self.status.capacity.set(self.game.capacity());
        self.close && registry.players.is_empty()
    }
}

/// `join` places a reserved player before its client learns the entity ID.
pub(super) fn register(
    registry: &mut Registry,
    request: Registration,
    join: impl FnOnce(&mut Registry, i32),
) {
    let result = match request.entity_id {
        Some(id) => registry.reserve_as(
            id,
            request.profile,
            request.transform,
            request.skin_parts,
            request.input,
        ),
        None => {
            registry.reserve(request.profile, request.transform, request.skin_parts, request.input)
        }
    };
    let reserved = result.as_ref().ok().copied();
    if let Some(id) = reserved {
        join(registry, id);
    }
    if request.reply.send(result).is_err()
        && let Some(id) = reserved
    {
        registry.remove(id);
    }
}

fn notify(registry: &Rc<RefCell<Registry>>, changed: bool, observers: &mut Vec<Rc<Input>>) {
    {
        let mut registry = registry.borrow_mut();
        if changed {
            registry.revision = registry.revision.wrapping_add(1);
        }
        observers.extend(
            registry
                .players
                .values()
                .filter(|player| player.active)
                .map(|player| player.input.clone()),
        );
    }
    // Do not wake application tasks while borrowing the published registry.
    for observer in observers.drain(..) {
        observer.changed.notify();
    }
}

pub(super) async fn run_owner(
    registry: Rc<RefCell<Registry>>,
    receiver: local_channel::LocalReceiver<Registration>,
    owner: Rc<Signal>,
    metrics: Rc<Metrics>,
    mut driver: Driver,
    diagnostics: bool,
) {
    let status = driver.status.clone();
    let map = driver.map.clone();
    let mut clock = SimulationClock::new(Instant::now());
    let mut report_at = Instant::now() + Duration::from_secs(5);
    let mut ticks = 0u64;
    let mut overruns = 0u64;
    let mut max_tick = Duration::ZERO;
    let mut registration_peak = 0usize;
    let mut observer_scratch = Vec::new();
    let mut self_notices = Vec::new();
    let mut cpu_samples = TimingSamples::default();
    let mut late_samples = TimingSamples::default();
    let mut skipped_steps = 0;
    let mut proposals = 0;
    let mut collision_cells = 0;
    let mut corrections = 0;
    enum Wake {
        Registration(Option<Registration>),
        Cleanup,
        Tick,
    }
    loop {
        let cleanup_changed = registry.borrow_mut().cleanup();
        if cleanup_changed {
            notify(&registry, true, &mut observer_scratch);
        }
        // Timer precedence prevents a registration flood starving simulation.
        let wake = futures_lite::future::or(
            async {
                Timer::new(clock.next_deadline().saturating_duration_since(Instant::now())).await;
                Wake::Tick
            },
            futures_lite::future::or(
                async {
                    owner.wait().await;
                    Wake::Cleanup
                },
                async { Wake::Registration(receiver.recv().await) },
            ),
        )
        .await;
        match wake {
            Wake::Registration(Some(request)) => {
                metrics.pending_registrations.set(metrics.pending_registrations.get() - 1);
                registration_peak = registration_peak.max(1);
                register(&mut registry.borrow_mut(), request, |registry, id| {
                    driver.join(registry, id, &mut self_notices);
                });
                for count in 1..REGISTRATIONS_PER_SLICE {
                    match futures_lite::future::poll_once(receiver.recv()).await {
                        Some(Some(request)) => {
                            metrics
                                .pending_registrations
                                .set(metrics.pending_registrations.get() - 1);
                            registration_peak = registration_peak.max(count + 1);
                            register(&mut registry.borrow_mut(), request, |registry, id| {
                                driver.join(registry, id, &mut self_notices);
                            });
                        }
                        _ => break,
                    }
                }
                for notice in self_notices.drain(..) {
                    notice.changed.notify();
                }
                glommio::executor().yield_now().await;
            }
            Wake::Registration(None) => break,
            Wake::Cleanup => {}
            Wake::Tick => {
                let due = clock.poll(Instant::now());
                skipped_steps += due.skipped_steps;
                late_samples.record(due.lateness);
                let mut finished = false;
                for _ in 0..due.steps {
                    let start = Instant::now();
                    let step = registry.borrow_mut().simulate(&map.world, start, &mut self_notices);
                    finished |= driver.step(&mut registry.borrow_mut(), &mut self_notices);
                    for notice in self_notices.drain(..) {
                        notice.changed.notify();
                    }
                    let elapsed = start.elapsed();
                    cpu_samples.record(elapsed);
                    ticks += 1;
                    max_tick = max_tick.max(elapsed);
                    overruns += u64::from(
                        elapsed.as_nanos()
                            > 1_000_000_000 / u128::from(crate::simulation::SIMULATION_HZ),
                    );
                    proposals += step.proposals;
                    collision_cells += step.collision_cells;
                    corrections += step.corrections;
                    glommio::executor().yield_now().await;
                }
                if due.publication {
                    let changed = registry.borrow_mut().publish();
                    // Only publication wakes all observers; immediate self updates
                    // above wake the affected connection without global fanout.
                    notify(&registry, changed, &mut observer_scratch);
                }
                if finished {
                    break;
                }
                if diagnostics && Instant::now() >= report_at {
                    let registry = registry.borrow();
                    let (cpu_p95, cpu_p99) = cpu_samples.percentiles();
                    let (late_p95, late_p99) = late_samples.percentiles();
                    let mut observers = ObserverMetrics::default();
                    for player in registry.players.values().filter(|player| player.active) {
                        observers.accumulate(player.input.observer_metrics.get());
                    }
                    eprintln!(
                        "instance {} {}: players={} reserved={} ticks={} max_tick_us={} cpu_p95_us={} cpu_p99_us={} lateness_p95_us={} lateness_p99_us={} overruns={} skipped_steps={} proposals={} collision_cells={} corrections={} registration_slice_peak={} registration_queue={} registration_queue_peak={} coalesced={} sent_bytes={} slow_clients={} visible_edges={} roster_entries={} queued_bytes={} queued_frames={} observer_high_water_bytes={} observer_coalesced={} observer_sent_bytes={}",
                        status.id,
                        status.kind,
                        registry.active_count(),
                        registry.players.len(),
                        ticks,
                        max_tick.as_micros(),
                        cpu_p95,
                        cpu_p99,
                        late_p95,
                        late_p99,
                        overruns,
                        skipped_steps,
                        proposals,
                        collision_cells,
                        corrections,
                        registration_peak,
                        metrics.pending_registrations.get(),
                        metrics.registration_peak.replace(0),
                        registry.coalesced,
                        metrics.sent_bytes.get(),
                        metrics.slow_clients.get(),
                        observers.visible,
                        observers.roster,
                        observers.queued_bytes,
                        observers.queued_frames,
                        observers.high_water_bytes,
                        observers.coalesced,
                        observers.sent_bytes,
                    );
                    report_at = Instant::now() + Duration::from_secs(5);
                    ticks = 0;
                    overruns = 0;
                    max_tick = Duration::ZERO;
                    registration_peak = 0;
                    skipped_steps = 0;
                    proposals = 0;
                    collision_cells = 0;
                    corrections = 0;
                    cpu_samples.clear();
                    late_samples.clear();
                }
                glommio::executor().yield_now().await;
            }
        }
    }
    status.closed.set(true);
}

struct TimingSamples {
    values: [u64; 1024],
    count: usize,
}

impl Default for TimingSamples {
    fn default() -> Self {
        Self { values: [0; 1024], count: 0 }
    }
}

impl TimingSamples {
    fn record(&mut self, elapsed: Duration) {
        self.values[self.count % self.values.len()] =
            elapsed.as_micros().min(u64::MAX as u128) as u64;
        self.count += 1;
    }

    fn percentiles(&self) -> (u64, u64) {
        let count = self.count.min(self.values.len());
        if count == 0 {
            return (0, 0);
        }
        let mut values = self.values;
        values[..count].sort_unstable();
        (values[(count * 95 / 100).min(count - 1)], values[(count * 99 / 100).min(count - 1)])
    }

    fn clear(&mut self) {
        self.count = 0;
    }
}
