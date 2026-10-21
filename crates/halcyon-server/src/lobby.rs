//! Human lobby state, published by one executor-local owner every 50 ms.
//! Connections publish replaceable input; dropping a session never needs queue space.
use glommio::channels::{local_channel, oneshot};
use glommio::timer::{MissedTickBehavior, interval};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const MAX_PLAYERS: usize = 1_024;
const REGISTRATION_CAPACITY: usize = 256;
const REGISTRATIONS_PER_SLICE: usize = 64;
const TICK_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Transform {
    pub fn chunk(self) -> (i32, i32) {
        ((self.x.floor() as i32) >> 4, (self.z.floor() as i32) >> 4)
    }

    fn is_finite(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.z.is_finite()
            && self.yaw.is_finite()
            && self.pitch.is_finite()
    }
}

#[derive(Debug)]
pub struct Profile {
    pub uuid: Uuid,
    pub username: String,
    /// Complete, preencoded Player Info Update frame, shared by all observers.
    pub info: Rc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct PlayerSnapshot {
    pub entity_id: i32,
    pub profile: Rc<Profile>,
    pub transform: Transform,
    pub skin_parts: u8,
    pub revision: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdmissionError {
    #[error("the lobby is full")]
    Full,
    #[error("this account is already connected")]
    DuplicateUuid,
    #[error("the lobby registration queue is full")]
    QueueFull,
    #[error("the lobby owner is unavailable")]
    Closed,
    #[error("the lobby entity ID space is exhausted")]
    IdExhausted,
    #[error("invalid initial player position")]
    InvalidTransform,
    #[error("invalid authenticated player profile")]
    InvalidProfile,
}

#[derive(Default)]
struct Signal {
    pending: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl Signal {
    fn notify(&self) {
        self.pending.set(true);
        let waker = self.waker.borrow_mut().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.pending.replace(false) {
            Poll::Ready(())
        } else {
            self.waker.borrow_mut().replace(cx.waker().clone());
            Poll::Pending
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| self.poll(cx)).await;
    }
}

struct Input {
    closed: Cell<bool>,
    activate: Cell<bool>,
    transform: Cell<Option<Transform>>,
    skin_parts: Cell<Option<u8>>,
    changed: Signal,
    owner: Rc<Signal>,
    coalesced: Cell<u64>,
}

impl Input {
    fn new(owner: Rc<Signal>) -> Self {
        Self {
            closed: Cell::new(false),
            activate: Cell::new(false),
            transform: Cell::new(None),
            skin_parts: Cell::new(None),
            changed: Signal::default(),
            owner,
            coalesced: Cell::new(0),
        }
    }
}

/// A unique connection lifetime guard. Deliberately not Clone.
pub struct Session {
    entity_id: i32,
    input: Rc<Input>,
}

impl Session {
    pub fn entity_id(&self) -> i32 {
        self.entity_id
    }

    /// Called after confirming the initial teleport. Visibility begins next tick.
    pub fn activate(&self) {
        self.input.activate.set(true);
    }

    pub fn publish_transform(&self, transform: Transform) {
        // Network handlers validate before publishing; this also protects future callers.
        if transform.is_finite() && self.input.transform.replace(Some(transform)).is_some() {
            self.input.coalesced.set(self.input.coalesced.get().saturating_add(1));
        }
    }

    pub fn publish_skin_parts(&self, skin_parts: u8) {
        self.input.skin_parts.set(Some(skin_parts));
    }

    pub fn poll_changed(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.input.changed.poll(cx)
    }

    /// A payload-free, coalesced wakeup; cancellation does not allocate event backlog.
    pub async fn wait_changed(&self) {
        std::future::poll_fn(|cx| self.poll_changed(cx)).await;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.input.closed.set(true);
        self.input.owner.notify();
    }
}

struct Player {
    snapshot: PlayerSnapshot,
    active: bool,
    input: Rc<Input>,
}

type ColumnMove = (i32, Option<(i32, i32)>, (i32, i32));

/// Read-only access to the current published lobby state.
#[derive(Default)]
pub struct Registry {
    players: HashMap<i32, Player>,
    uuids: HashSet<Uuid>,
    columns: HashMap<(i32, i32), HashSet<i32>>,
    last_entity_id: i32,
    active_count: usize,
    revision: u64,
    roster_revision: u64,
    spatial_revision: u64,
    publication: u64,
    coalesced: u64,
    closed_scratch: Vec<i32>,
    moved_scratch: Vec<ColumnMove>,
}

impl Registry {
    #[cfg(test)]
    pub(crate) fn from_snapshots(players: Vec<PlayerSnapshot>, publication: u64) -> Self {
        let mut registry = Self {
            revision: publication,
            roster_revision: publication,
            spatial_revision: publication,
            publication,
            ..Self::default()
        };
        for snapshot in players {
            registry.last_entity_id = registry.last_entity_id.max(snapshot.entity_id);
            registry.uuids.insert(snapshot.profile.uuid);
            registry
                .columns
                .entry(snapshot.transform.chunk())
                .or_default()
                .insert(snapshot.entity_id);
            registry.players.insert(
                snapshot.entity_id,
                Player { snapshot, active: true, input: Rc::new(Input::new(Rc::default())) },
            );
        }
        registry.active_count = registry.players.len();
        registry
    }

    pub fn players(&self) -> impl Iterator<Item = &PlayerSnapshot> {
        self.players.values().filter(|player| player.active).map(|player| &player.snapshot)
    }

    pub fn get(&self, entity_id: i32) -> Option<&PlayerSnapshot> {
        self.players.get(&entity_id).filter(|player| player.active).map(|player| &player.snapshot)
    }

    pub fn candidates(&self, columns: &HashSet<(i32, i32)>) -> Vec<PlayerSnapshot> {
        let mut result = Vec::new();
        for column in columns {
            if let Some(ids) = self.columns.get(column) {
                result.extend(ids.iter().filter_map(|id| self.get(*id)).cloned());
            }
        }
        result.sort_unstable_by_key(|player| player.entity_id);
        result
    }

    pub fn active_count(&self) -> usize {
        self.active_count
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn roster_revision(&self) -> u64 {
        self.roster_revision
    }

    pub fn spatial_revision(&self) -> u64 {
        self.spatial_revision
    }

    /// Advances on every publication, including ticks without player changes.
    pub fn publication(&self) -> u64 {
        self.publication
    }

    fn reserve(
        &mut self,
        profile: Rc<Profile>,
        transform: Transform,
        skin_parts: u8,
        input: Rc<Input>,
    ) -> Result<i32, AdmissionError> {
        if input.closed.get() {
            return Err(AdmissionError::Closed);
        }
        if !transform.is_finite() {
            return Err(AdmissionError::InvalidTransform);
        }
        if profile.username.is_empty()
            || profile.username.len() > 16
            || !profile.username.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(AdmissionError::InvalidProfile);
        }
        if self.uuids.contains(&profile.uuid) {
            return Err(AdmissionError::DuplicateUuid);
        }
        if self.players.len() >= MAX_PLAYERS {
            return Err(AdmissionError::Full);
        }
        let entity_id = self.last_entity_id.checked_add(1).ok_or(AdmissionError::IdExhausted)?;
        self.last_entity_id = entity_id;
        self.uuids.insert(profile.uuid);
        self.players.insert(
            entity_id,
            Player {
                snapshot: PlayerSnapshot { entity_id, profile, transform, skin_parts, revision: 0 },
                active: false,
                input,
            },
        );
        Ok(entity_id)
    }

    fn remove(&mut self, entity_id: i32) -> bool {
        let Some(player) = self.players.remove(&entity_id) else {
            return false;
        };
        self.uuids.remove(&player.snapshot.profile.uuid);
        if player.active {
            self.remove_column(player.snapshot.transform.chunk(), entity_id);
            self.active_count -= 1;
            self.roster_revision = self.roster_revision.wrapping_add(1);
            self.spatial_revision = self.spatial_revision.wrapping_add(1);
            true
        } else {
            false
        }
    }

    fn remove_column(&mut self, column: (i32, i32), entity_id: i32) {
        if let Some(ids) = self.columns.get_mut(&column) {
            ids.remove(&entity_id);
            if ids.is_empty() {
                self.columns.remove(&column);
            }
        }
    }

    fn cleanup(&mut self) -> bool {
        self.closed_scratch.extend(
            self.players.iter().filter(|(_, player)| player.input.closed.get()).map(|(&id, _)| id),
        );
        let mut changed = false;
        while let Some(id) = self.closed_scratch.pop() {
            changed |= self.remove(id);
        }
        changed
    }

    fn publish(&mut self) -> bool {
        self.publication = self.publication.wrapping_add(1);
        let mut changed = false;
        // Apply all latest input once, rather than scheduling one task per packet.
        for (&id, player) in &mut self.players {
            self.coalesced = self.coalesced.saturating_add(player.input.coalesced.replace(0));
            let old_column = player.snapshot.transform.chunk();
            let mut player_changed = false;
            if let Some(transform) = player.input.transform.take()
                && transform != player.snapshot.transform
            {
                player.snapshot.transform = transform;
                player_changed = true;
            }
            if let Some(skin_parts) = player.input.skin_parts.take()
                && skin_parts != player.snapshot.skin_parts
            {
                player.snapshot.skin_parts = skin_parts;
                player_changed = true;
            }
            let activated = player.input.activate.replace(false) && !player.active;
            if activated {
                player.active = true;
                self.active_count += 1;
                self.roster_revision = self.roster_revision.wrapping_add(1);
                player_changed = true;
            }
            let new_column = player.snapshot.transform.chunk();
            if player.active && (activated || old_column != new_column) {
                self.moved_scratch.push((id, (!activated).then_some(old_column), new_column));
            }
            if player_changed {
                player.snapshot.revision = player.snapshot.revision.wrapping_add(1);
                changed |= player.active;
            }
        }
        if !self.moved_scratch.is_empty() {
            self.spatial_revision = self.spatial_revision.wrapping_add(1);
        }
        while let Some((id, old, new)) = self.moved_scratch.pop() {
            if let Some(old) = old {
                self.remove_column(old, id);
            }
            self.columns.entry(new).or_default().insert(id);
        }
        changed
    }
}

#[derive(Default)]
struct Metrics {
    sent_bytes: Cell<u64>,
    slow_clients: Cell<u64>,
    pending_registrations: Cell<usize>,
    registration_peak: Cell<usize>,
}

struct Registration {
    profile: Rc<Profile>,
    transform: Transform,
    skin_parts: u8,
    input: Rc<Input>,
    reply: oneshot::Sender<Result<i32, AdmissionError>>,
}

#[derive(Clone)]
pub struct Lobby {
    registry: Rc<RefCell<Registry>>,
    registrations: Rc<local_channel::LocalSender<Registration>>,
    owner: Rc<Signal>,
    metrics: Rc<Metrics>,
}

impl Lobby {
    pub fn spawn(diagnostics: bool) -> Self {
        let registry = Rc::new(RefCell::new(Registry::default()));
        let (sender, receiver) = local_channel::new_bounded(REGISTRATION_CAPACITY);
        let owner = Rc::new(Signal::default());
        let metrics = Rc::new(Metrics::default());
        glommio::spawn_local(run_owner(
            registry.clone(),
            receiver,
            owner.clone(),
            metrics.clone(),
            diagnostics,
        ))
        .detach();
        Self { registry, registrations: Rc::new(sender), owner, metrics }
    }

    pub fn with_registry<T>(&self, read: impl FnOnce(&Registry) -> T) -> T {
        read(&self.registry.borrow())
    }

    pub fn active_count(&self) -> usize {
        self.registry.borrow().active_count()
    }

    pub fn record_sent(&self, bytes: usize) {
        self.metrics.sent_bytes.set(self.metrics.sent_bytes.get().saturating_add(bytes as u64));
    }

    pub fn record_slow_client(&self) {
        self.metrics.slow_clients.set(self.metrics.slow_clients.get().saturating_add(1));
    }

    pub async fn register(
        &self,
        profile: Rc<Profile>,
        transform: Transform,
        skin_parts: u8,
    ) -> Result<Session, AdmissionError> {
        let input = Rc::new(Input::new(self.owner.clone()));
        // The guard exists across registration cancellation, even before an ID arrives.
        let mut session = Session { entity_id: 0, input: input.clone() };
        let (reply, response) = oneshot::oneshot();
        self.registrations
            .try_send(Registration { profile, transform, skin_parts, input, reply })
            .map_err(|_| AdmissionError::QueueFull)?;
        let queued = self.metrics.pending_registrations.get() + 1;
        self.metrics.pending_registrations.set(queued);
        self.metrics.registration_peak.set(self.metrics.registration_peak.get().max(queued));
        session.entity_id = response.await.map_err(|_| AdmissionError::Closed)??;
        Ok(session)
    }
}

fn register(registry: &mut Registry, request: Registration) {
    let result =
        registry.reserve(request.profile, request.transform, request.skin_parts, request.input);
    let reserved = result.as_ref().ok().copied();
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

async fn run_owner(
    registry: Rc<RefCell<Registry>>,
    receiver: local_channel::LocalReceiver<Registration>,
    owner: Rc<Signal>,
    metrics: Rc<Metrics>,
    diagnostics: bool,
) {
    let mut ticker = interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut report_at = Instant::now() + Duration::from_secs(5);
    let mut ticks = 0u64;
    let mut overruns = 0u64;
    let mut max_tick = Duration::ZERO;
    let mut registration_peak = 0usize;
    let mut observer_scratch = Vec::new();
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
        // Timer precedence prevents a continuous registration flood starving publication.
        let wake = futures_lite::future::or(
            async {
                ticker.tick().await;
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
                register(&mut registry.borrow_mut(), request);
                for count in 1..REGISTRATIONS_PER_SLICE {
                    match futures_lite::future::poll_once(receiver.recv()).await {
                        Some(Some(request)) => {
                            metrics
                                .pending_registrations
                                .set(metrics.pending_registrations.get() - 1);
                            registration_peak = registration_peak.max(count + 1);
                            register(&mut registry.borrow_mut(), request);
                        }
                        _ => break,
                    }
                }
                glommio::executor().yield_now().await;
            }
            Wake::Registration(None) => break,
            Wake::Cleanup => {}
            Wake::Tick => {
                let start = Instant::now();
                let changed = registry.borrow_mut().publish();
                // Wake once per tick so bounded visibility admission can continue even
                // when no player has moved. Notifications coalesce in each session.
                notify(&registry, changed, &mut observer_scratch);
                let elapsed = start.elapsed();
                ticks += 1;
                max_tick = max_tick.max(elapsed);
                overruns += u64::from(elapsed > TICK_INTERVAL);
                if diagnostics && Instant::now() >= report_at {
                    let registry = registry.borrow();
                    eprintln!(
                        "lobby: players={} reserved={} ticks={} max_tick_us={} overruns={} registration_slice_peak={} registration_queue={} registration_queue_peak={} coalesced={} sent_bytes={} slow_clients={}",
                        registry.active_count(),
                        registry.players.len(),
                        ticks,
                        max_tick.as_micros(),
                        overruns,
                        registration_peak,
                        metrics.pending_registrations.get(),
                        metrics.registration_peak.replace(0),
                        registry.coalesced,
                        metrics.sent_bytes.get(),
                        metrics.slow_clients.get()
                    );
                    report_at = Instant::now() + Duration::from_secs(5);
                    ticks = 0;
                    overruns = 0;
                    max_tick = Duration::ZERO;
                    registration_peak = 0;
                }
                glommio::executor().yield_now().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(uuid: u128) -> Rc<Profile> {
        Rc::new(Profile {
            uuid: Uuid::from_u128(uuid),
            username: "Player".into(),
            info: Rc::from([]),
        })
    }

    fn reserve(registry: &mut Registry, uuid: u128) -> Session {
        let input = Rc::new(Input::new(Rc::default()));
        let id = registry.reserve(profile(uuid), Transform::default(), 0, input.clone()).unwrap();
        Session { entity_id: id, input }
    }

    #[test]
    fn activation_coalesces_input_and_updates_spatial_membership() {
        let mut registry = Registry::default();
        let session = reserve(&mut registry, 1);
        assert!(registry.get(session.entity_id()).is_none());
        assert_eq!(registry.active_count(), 0);
        session.publish_transform(Transform { x: -0.5, ..Transform::default() });
        session.publish_transform(Transform { x: 17.0, ..Transform::default() });
        session.publish_skin_parts(0x7f);
        session.activate();
        assert!(registry.publish());
        let snapshot = registry.get(session.entity_id()).unwrap();
        assert_eq!(snapshot.transform.x, 17.0);
        assert_eq!(snapshot.skin_parts, 0x7f);
        assert_eq!(snapshot.revision, 1);
        assert_eq!(registry.candidates(&HashSet::from([(1, 0)])).len(), 1);
        assert!(registry.candidates(&HashSet::from([(-1, 0)])).is_empty());
        assert!(!registry.publish());
        session.publish_transform(Transform { x: -0.5, ..Transform::default() });
        assert!(registry.publish());
        assert!(registry.candidates(&HashSet::from([(1, 0)])).is_empty());
        assert_eq!(registry.candidates(&HashSet::from([(-1, 0)])).len(), 1);
        let fixture = Registry::from_snapshots(registry.players().cloned().collect(), 2);
        assert_eq!(fixture.publication(), 2);
        assert_eq!(fixture.candidates(&HashSet::from([(-1, 0)])).len(), 1);
    }

    #[test]
    fn dropping_guards_cleans_up_without_enqueuing_and_ids_are_not_reused() {
        let mut registry = Registry::default();
        let first = reserve(&mut registry, 1);
        let first_id = first.entity_id();
        first.activate();
        registry.publish();
        assert!(matches!(
            registry.reserve(
                profile(1),
                Transform::default(),
                0,
                Rc::new(Input::new(Rc::default()))
            ),
            Err(AdmissionError::DuplicateUuid)
        ));
        drop(first);
        assert!(registry.cleanup());
        assert_eq!(registry.active_count(), 0);
        assert!(registry.columns.is_empty());
        let second = reserve(&mut registry, 1);
        assert!(second.entity_id() > first_id);
        drop(second);
        assert!(!registry.cleanup());
        assert!(registry.players.is_empty());
    }

    #[test]
    fn admission_bounds_include_invisible_reservations_and_abandoned_replies() {
        let mut registry = Registry::default();
        let sessions: Vec<_> =
            (1..=MAX_PLAYERS).map(|uuid| reserve(&mut registry, uuid as u128)).collect();
        let input = Rc::new(Input::new(Rc::default()));
        assert_eq!(
            registry.reserve(profile(2_000), Transform::default(), 0, input),
            Err(AdmissionError::Full)
        );
        drop(sessions);
        registry.cleanup();
        let (reply, response) = oneshot::oneshot();
        drop(response);
        register(
            &mut registry,
            Registration {
                profile: profile(1),
                transform: Transform::default(),
                skin_parts: 0,
                input: Rc::new(Input::new(Rc::default())),
                reply,
            },
        );
        assert!(registry.players.is_empty());
        registry.last_entity_id = i32::MAX;
        assert_eq!(
            registry.reserve(
                profile(1),
                Transform::default(),
                0,
                Rc::new(Input::new(Rc::default()))
            ),
            Err(AdmissionError::IdExhausted)
        );
    }

    #[test]
    fn wakeups_coalesce_and_survive_a_cancelled_pending_wait() {
        let signal = Signal::default();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(signal.poll(&mut cx).is_pending());
        // Dropping the waiting future does not consume a later notification.
        signal.notify();
        signal.notify();
        assert!(signal.poll(&mut cx).is_ready());
        assert!(signal.poll(&mut cx).is_pending());
    }

    #[test]
    fn a_full_registration_queue_cannot_block_disconnect_cleanup() {
        let mut registry = Registry::default();
        let session = reserve(&mut registry, 1);
        session.activate();
        registry.publish();
        let (sender, _receiver) = local_channel::new_bounded(REGISTRATION_CAPACITY);
        for id in 0..REGISTRATION_CAPACITY {
            let (reply, _response) = oneshot::oneshot();
            let request = Registration {
                profile: profile(id as u128 + 2),
                transform: Transform::default(),
                skin_parts: 0,
                input: Rc::new(Input::new(Rc::default())),
                reply,
            };
            assert!(sender.try_send(request).is_ok());
        }
        assert!(sender.is_full());
        drop(session);
        assert!(registry.cleanup());
        assert_eq!(registry.active_count(), 0);
        assert!(registry.players.is_empty());
        assert!(sender.is_full());
    }

    #[test]
    #[ignore = "opt-in synthetic state-publication measurement; no sockets or visibility fan-out"]
    fn measure_state_publication() {
        for population in [128, MAX_PLAYERS] {
            let mut registry = Registry::default();
            let sessions: Vec<_> =
                (1..=population).map(|id| reserve(&mut registry, id as u128)).collect();
            for session in &sessions {
                session.activate();
            }
            registry.publish();
            let start = Instant::now();
            for tick in 0..200 {
                for (index, session) in sessions.iter().enumerate() {
                    session.publish_transform(Transform {
                        x: index as f64 + tick as f64 * 0.1,
                        ..Transform::default()
                    });
                }
                registry.publish();
            }
            eprintln!(
                "state population={population} moving_ticks=200 mean_tick_us={:.1} player_struct_bytes={} (excludes allocations, profiles, observers and sockets)",
                start.elapsed().as_secs_f64() * 1_000_000.0 / 200.0,
                std::mem::size_of::<Player>()
            );
        }
    }
}
