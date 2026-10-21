//! Connection lifetime guard, bounded input mailboxes and coalesced wakeups.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use glommio::channels::oneshot;
use mc_protocol::types::ItemStack;

use super::{ObserverMetrics, Transform};
use crate::inventory::InventoryAction;
use crate::lifecycle::{Damage, PlayerRules};
use crate::movement::{MovementProposal, ProposalQueue};
use crate::player::{ActionQueue, PlayerAction, PlayerUpdate};

#[derive(Default)]
pub(super) struct Signal {
    pending: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl Signal {
    pub(super) fn notify(&self) {
        self.pending.set(true);
        let waker = self.waker.borrow_mut().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(super) fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.pending.replace(false) {
            Poll::Ready(())
        } else {
            self.waker.borrow_mut().replace(cx.waker().clone());
            Poll::Pending
        }
    }

    pub(super) async fn wait(&self) {
        std::future::poll_fn(|cx| self.poll(cx)).await;
    }
}

pub(super) struct Input {
    pub(super) closed: Cell<bool>,
    pub(super) activate: Cell<bool>,
    pub(super) transform: Cell<Option<Transform>>,
    pub(super) proposals: RefCell<ProposalQueue>,
    pub(super) actions: RefCell<ActionQueue>,
    pub(super) accepted: Cell<Transform>,
    pub(super) generation: Cell<u64>,
    pub(super) ready: Cell<bool>,
    pub(super) teleport_confirmation: Cell<Option<i32>>,
    pub(super) respawn_requested: Cell<bool>,
    pub(super) update: RefCell<PlayerUpdate>,
    pub(super) skin_parts: Cell<Option<u8>>,
    pub(super) changed: Signal,
    owner: Rc<Signal>,
    pub(super) coalesced: Cell<u64>,
    pub(super) observer_metrics: Cell<ObserverMetrics>,
}

impl Input {
    pub(super) fn new(owner: Rc<Signal>) -> Self {
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
    pub(super) entity_id: i32,
    pub(super) input: Rc<Input>,
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
