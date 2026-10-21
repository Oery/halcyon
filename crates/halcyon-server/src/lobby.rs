//! One executor-local gameplay owner: 20 Hz actions and observer publication.
//! Dropping a session never needs queue space.
use glommio::channels::{local_channel, oneshot};
use glommio::timer::Timer;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::inventory::InventoryAction;
use crate::lifecycle::{Damage, PlayerRules};
use crate::movement::{MovementProposal, ProposalQueue};
use crate::player::{ActionQueue, PlayerAction, PlayerState, PlayerUpdate};
use crate::simulation::SimulationClock;
use mc_protocol::types::ItemStack;

pub const MAX_PLAYERS: usize = 1_024;
const REGISTRATION_CAPACITY: usize = 256;
const REGISTRATIONS_PER_SLICE: usize = 64;

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

    /// Coordinate domain shared by movement, teleports and spawn destinations.
    pub(crate) fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.z.is_finite()
            && self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.x.abs() <= 30_000_000.0
            && self.z.abs() <= 30_000_000.0
            && self.y.abs() <= 20_000_000.0
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
    pub sneaking: bool,
    pub crouching: bool,
    pub sprinting: bool,
    pub health: f32,
    pub equipment: Rc<[ItemStack; 6]>,
    pub life_generation: u64,
    pub visible: bool,
    pub revision: u64,
}

#[derive(Clone, Copy, Default)]
pub struct ObserverMetrics {
    pub visible: usize,
    pub roster: usize,
    pub queued_bytes: usize,
    pub queued_frames: usize,
    pub high_water_bytes: usize,
    pub coalesced: u64,
    pub sent_bytes: u64,
}

impl ObserverMetrics {
    fn accumulate(&mut self, observer: Self) {
        self.visible = self.visible.saturating_add(observer.visible);
        self.roster = self.roster.saturating_add(observer.roster);
        self.queued_bytes = self.queued_bytes.saturating_add(observer.queued_bytes);
        self.queued_frames = self.queued_frames.saturating_add(observer.queued_frames);
        self.high_water_bytes = self.high_water_bytes.max(observer.high_water_bytes);
        self.coalesced = self.coalesced.saturating_add(observer.coalesced);
        self.sent_bytes = self.sent_bytes.saturating_add(observer.sent_bytes);
    }
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
    proposals: RefCell<ProposalQueue>,
    actions: RefCell<ActionQueue>,
    accepted: Cell<Transform>,
    generation: Cell<u64>,
    ready: Cell<bool>,
    teleport_confirmation: Cell<Option<i32>>,
    respawn_requested: Cell<bool>,
    update: RefCell<PlayerUpdate>,
    skin_parts: Cell<Option<u8>>,
    changed: Signal,
    owner: Rc<Signal>,
    coalesced: Cell<u64>,
    observer_metrics: Cell<ObserverMetrics>,
}

impl Input {
    fn new(owner: Rc<Signal>) -> Self {
        Self {
            closed: Cell::new(false),
            activate: Cell::new(false),
            transform: Cell::new(None),
            proposals: RefCell::new(ProposalQueue::default()),
            actions: RefCell::new(ActionQueue::default()),
            accepted: Cell::new(Transform::default()),
            generation: Cell::new(0),
            ready: Cell::new(false),
            teleport_confirmation: Cell::new(None),
            respawn_requested: Cell::new(false),
            update: RefCell::new(PlayerUpdate::default()),
            skin_parts: Cell::new(None),
            changed: Signal::default(),
            owner,
            coalesced: Cell::new(0),
            observer_metrics: Cell::new(ObserverMetrics::default()),
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
    #[cfg(test)]
    pub fn activate(&self) {
        self.input.activate.set(true);
    }

    #[cfg(test)]
    pub fn publish_transform(&self, transform: Transform) {
        // Network handlers validate before publishing; this also protects future callers.
        if transform.is_valid() && self.input.transform.replace(Some(transform)).is_some() {
            self.input.coalesced.set(self.input.coalesced.get().saturating_add(1));
        }
    }

    pub fn propose_movement(&self, proposal: MovementProposal) {
        if self.input.ready.get() {
            self.input.proposals.borrow_mut().push(proposal);
        }
    }

    /// Trusted gameplay API for future server-driven participants. Network
    /// PlayerInput packets do not opt human sessions into a second simulation.
    #[allow(dead_code)]
    pub fn drive(&self, input: Option<crate::movement::GroundInput>) -> bool {
        self.ready() && self.submit(PlayerAction::Drive(input))
    }

    pub fn accepted_transform(&self) -> Transform {
        self.input.accepted.get()
    }

    pub fn ready(&self) -> bool {
        self.input.ready.get()
    }

    /// Acknowledgements cannot compete with inventory actions for queue space.
    pub fn confirm_teleport(&self, id: i32) {
        self.input.teleport_confirmation.set(Some(id));
    }

    pub fn take_update(&self) -> PlayerUpdate {
        std::mem::take(&mut *self.input.update.borrow_mut())
    }

    pub fn submit(&self, action: PlayerAction) -> bool {
        self.input.actions.borrow_mut().push(self.input.generation.get(), action)
    }

    pub fn inventory(&self, action: InventoryAction) {
        self.submit(PlayerAction::Inventory(action));
    }

    // Arena APIs are intentionally available before a concrete minigame consumes them.
    #[allow(dead_code)]
    pub fn teleport(&self, transform: Transform) -> bool {
        transform.is_valid() && self.submit(PlayerAction::Teleport(transform))
    }

    #[allow(dead_code)]
    pub fn damage(&self, damage: Damage) -> bool {
        self.submit(PlayerAction::Damage(damage))
    }

    #[allow(dead_code)]
    pub fn heal(&self, amount: f32) -> bool {
        self.submit(PlayerAction::Heal(amount))
    }

    #[allow(dead_code)]
    pub fn set_rules(&self, rules: PlayerRules) -> bool {
        rules.respawn.is_valid() && self.submit(PlayerAction::Rules(rules))
    }

    pub fn respawn(&self) {
        self.input.respawn_requested.set(true);
    }

    #[allow(dead_code)]
    pub fn reset(&self) -> bool {
        self.submit(PlayerAction::Reset)
    }

    /// An error performs no inventory edit. Give returns the unplaced remainder.
    #[allow(dead_code)]
    pub async fn edit_inventory(
        &self,
        edit: crate::player::InventoryEdit,
    ) -> Result<ItemStack, crate::player::InventoryEditError> {
        use crate::player::InventoryEditError;
        let (reply, response) = oneshot::oneshot();
        if !self.submit(PlayerAction::EditInventory { edit, reply }) {
            return Err(InventoryEditError::QueueFull);
        }
        response.await.map_err(|_| InventoryEditError::Closed)?
    }

    pub fn publish_skin_parts(&self, skin_parts: u8) {
        self.input.skin_parts.set(Some(skin_parts));
    }

    pub fn record_observer(&self, metrics: ObserverMetrics) {
        self.input.observer_metrics.set(metrics);
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
    state: PlayerState,
}

type ColumnMove = (i32, Option<(i32, i32)>, (i32, i32));

/// Read-only access to the current published lobby state.
#[derive(Default)]
pub struct Registry {
    pub arena: crate::arena::Arena,
    world_actions: std::collections::VecDeque<WorldAction>,
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
                Player {
                    state: PlayerState::new(snapshot.transform),
                    snapshot,
                    active: true,
                    input: Rc::new(Input::new(Rc::default())),
                },
            );
        }
        registry.active_count = registry.players.len();
        registry
    }

    #[cfg(test)]
    pub(crate) fn publish_transforms(
        &mut self,
        transforms: impl IntoIterator<Item = (i32, Transform)>,
    ) {
        for (id, transform) in transforms {
            if let Some(player) = self.players.get(&id)
                && transform.is_valid()
            {
                player.input.transform.set(Some(transform));
            }
        }
        if self.publish() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Estimated owned container payload capacity and live input/profile payloads.
    /// Shared profiles are counted once per player, never once per observer.
    /// Excludes Registry itself, Rc headers, hash controls, allocator overhead,
    /// owner-task storage and sockets; this is neither RSS nor total memory.
    #[cfg(test)]
    pub(crate) fn capacity_payload_bytes(&self) -> usize {
        self.arena.capacity_payload_bytes()
            + self.players.capacity() * std::mem::size_of::<(i32, Player)>()
            + self.uuids.capacity() * std::mem::size_of::<Uuid>()
            + self.columns.capacity() * std::mem::size_of::<((i32, i32), HashSet<i32>)>()
            + self
                .columns
                .values()
                .map(|ids| ids.capacity() * std::mem::size_of::<i32>())
                .sum::<usize>()
            + self.closed_scratch.capacity() * std::mem::size_of::<i32>()
            + self.moved_scratch.capacity() * std::mem::size_of::<ColumnMove>()
            + self
                .players
                .values()
                .map(|player| {
                    std::mem::size_of::<Input>()
                        + std::mem::size_of::<Profile>()
                        + player.state.movement.capacity_payload_bytes()
                        + player.input.proposals.borrow().capacity_payload_bytes()
                        + player.input.actions.borrow().capacity_payload_bytes()
                        + std::mem::size_of::<[ItemStack; 6]>()
                        + player.snapshot.profile.username.capacity()
                        + player.snapshot.profile.info.len()
                })
                .sum::<usize>()
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
                result.extend(
                    ids.iter()
                        .filter_map(|id| self.get(*id))
                        .filter(|player| player.visible)
                        .cloned(),
                );
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
        if !transform.is_valid() {
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
        input.accepted.set(transform);
        self.players.insert(
            entity_id,
            Player {
                snapshot: PlayerSnapshot {
                    entity_id,
                    profile,
                    transform,
                    skin_parts,
                    revision: 0,
                    sneaking: false,
                    crouching: false,
                    sprinting: false,
                    health: 20.0,
                    equipment: Rc::new(std::array::from_fn(|_| ItemStack::default())),
                    life_generation: 0,
                    visible: true,
                },
                active: false,
                input,
                state: PlayerState::new(transform),
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

    /// Gameplay mutates authority here; publication consumes only the latest result.
    fn simulate(
        &mut self,
        world: &halcyon_world::World,
        now: Instant,
        notices: &mut Vec<Rc<Input>>,
    ) -> StepMetrics {
        let mut metrics = StepMetrics::default();
        for _ in 0..64 {
            let Some(action) = self.world_actions.pop_front() else {
                break;
            };
            let result = self.arena.edit(world, action.pos, action.state);
            let _ = action.reply.send(result);
        }
        self.arena.step(world, &mut self.last_entity_id);
        let world = self.arena.view(world);
        for player in self.players.values_mut() {
            let input = &player.input;
            let had_teleport = player.state.life.pending_teleport().is_some();
            if let Some(id) = input.teleport_confirmation.take() {
                player.state.life.confirm(id);
            }
            let (update, movement) = player.state.advance(
                &world,
                &mut input.proposals.borrow_mut(),
                &mut input.actions.borrow_mut(),
                now,
                input.respawn_requested.replace(false),
            );
            let accepted = player.state.movement.transform();
            let old = input.accepted.replace(accepted);
            input.generation.set(player.state.life.generation);
            if had_teleport && player.state.life.pending_teleport().is_none() {
                input.activate.set(true);
            }
            input.ready.set(
                player.state.life.alive()
                    && player.state.life.pending_teleport().is_none()
                    && (player.active || input.activate.get()),
            );
            if old != accepted {
                input.transform.set(Some(accepted));
            }
            if let Some(movement) = movement {
                metrics.proposals += movement.processed as u64;
                metrics.collision_cells += movement.collision_cells as u64;
                metrics.corrections += u64::from(movement.correction);
            }
            if update.changed() || old.chunk() != accepted.chunk() {
                input.update.borrow_mut().merge(update);
                notices.push(input.clone());
            }
        }
        metrics
    }

    fn publish(&mut self) -> bool {
        self.publish_at(Instant::now())
    }

    fn publish_at(&mut self, now: Instant) -> bool {
        self.publication = self.publication.wrapping_add(1);
        let mut changed = false;
        // Apply all latest input once, rather than scheduling one task per packet.
        for (&id, player) in &mut self.players {
            self.coalesced = self.coalesced.saturating_add(player.input.coalesced.replace(0));
            let old_column = player.snapshot.transform.chunk();
            let mut player_changed = false;
            let sneaking = player.state.movement.sneaking();
            let crouching = player.state.movement.crouching();
            let sprinting = player.state.movement.sprinting();
            let health = player.state.life.health();
            let visible = player.state.life.visible(now);
            let generation = player.state.life.generation;
            if player.snapshot.sneaking != sneaking
                || player.snapshot.crouching != crouching
                || player.snapshot.sprinting != sprinting
                || player.snapshot.health != health
                || player.snapshot.visible != visible
                || player.snapshot.life_generation != generation
                || !Rc::ptr_eq(&player.snapshot.equipment, &player.state.equipment)
            {
                if player.snapshot.visible != visible
                    || player.snapshot.life_generation != generation
                {
                    self.spatial_revision = self.spatial_revision.wrapping_add(1);
                }
                player.snapshot.sneaking = sneaking;
                player.snapshot.crouching = crouching;
                player.snapshot.sprinting = sprinting;
                player.snapshot.health = health;
                player.snapshot.visible = visible;
                player.snapshot.life_generation = generation;
                player.snapshot.equipment = player.state.equipment.clone();
                player_changed = true;
            }
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

#[derive(Default)]
struct StepMetrics {
    proposals: u64,
    collision_cells: u64,
    corrections: u64,
}

struct Registration {
    profile: Rc<Profile>,
    transform: Transform,
    skin_parts: u8,
    input: Rc<Input>,
    reply: oneshot::Sender<Result<i32, AdmissionError>>,
}

struct WorldAction {
    pos: crate::arena::BlockPos,
    state: halcyon_world::BlockStateId,
    reply: oneshot::Sender<Result<(), crate::arena::EditError>>,
}

#[derive(Clone)]
pub struct Lobby {
    registry: Rc<RefCell<Registry>>,
    registrations: Rc<local_channel::LocalSender<Registration>>,
    owner: Rc<Signal>,
    metrics: Rc<Metrics>,
}

impl Lobby {
    pub fn spawn(
        world: Arc<halcyon_world::World>,
        diagnostics: bool,
    ) -> Result<Self, crate::arena::EditError> {
        let mut state = Registry::default();
        state.arena.initialize(&world)?;
        let registry = Rc::new(RefCell::new(state));
        let (sender, receiver) = local_channel::new_bounded(REGISTRATION_CAPACITY);
        let owner = Rc::new(Signal::default());
        let metrics = Rc::new(Metrics::default());
        glommio::spawn_local(run_owner(
            registry.clone(),
            receiver,
            owner.clone(),
            metrics.clone(),
            diagnostics,
            world,
        ))
        .detach();
        Ok(Self { registry, registrations: Rc::new(sender), owner, metrics })
    }

    /// Trusted arena edits only; player digging and placement remain disabled.
    #[allow(dead_code)]
    pub async fn set_block(
        &self,
        pos: crate::arena::BlockPos,
        state: halcyon_world::BlockStateId,
    ) -> Result<(), crate::arena::EditError> {
        let (reply, response) = oneshot::oneshot();
        {
            let mut registry = self.registry.borrow_mut();
            if registry.world_actions.len() == 64 {
                return Err(crate::arena::EditError::Unavailable);
            }
            registry.world_actions.push_back(WorldAction { pos, state, reply });
        }
        response.await.map_err(|_| crate::arena::EditError::Unavailable)?
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
    world: Arc<halcyon_world::World>,
) {
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
                let due = clock.poll(Instant::now());
                skipped_steps += due.skipped_steps;
                late_samples.record(due.lateness);
                for _ in 0..due.steps {
                    let start = Instant::now();
                    let step = registry.borrow_mut().simulate(&world, start, &mut self_notices);
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
                if diagnostics && Instant::now() >= report_at {
                    let registry = registry.borrow();
                    let (cpu_p95, cpu_p99) = cpu_samples.percentiles();
                    let (late_p95, late_p99) = late_samples.percentiles();
                    let mut observers = ObserverMetrics::default();
                    for player in registry.players.values().filter(|player| player.active) {
                        observers.accumulate(player.input.observer_metrics.get());
                    }
                    eprintln!(
                        "lobby: players={} reserved={} ticks={} max_tick_us={} cpu_p95_us={} cpu_p99_us={} lateness_p95_us={} lateness_p99_us={} overruns={} skipped_steps={} proposals={} collision_cells={} corrections={} registration_slice_peak={} registration_queue={} registration_queue_peak={} coalesced={} sent_bytes={} slow_clients={} visible_edges={} roster_entries={} queued_bytes={} queued_frames={} observer_high_water_bytes={} observer_coalesced={} observer_sent_bytes={}",
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

#[cfg(test)]
#[path = "bench/gameplay.rs"]
mod gameplay_benchmarks;

#[cfg(test)]
mod tests;
