//! One executor-local gameplay owner: 20 Hz actions and observer publication.
//! Dropping a session never needs queue space.
use glommio::channels::{local_channel, oneshot};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use uuid::Uuid;

use mc_protocol::types::ItemStack;

mod owner;
mod registry;
mod session;

use owner::run_owner;
pub use registry::Registry;
pub use session::Session;
use session::{Input, Signal};

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

#[cfg(test)]
#[path = "bench/gameplay.rs"]
mod gameplay_benchmarks;

#[cfg(test)]
#[path = "tests/tests_lobby.rs"]
mod tests;
