//! Authoritative per-player gameplay. No packet encoding or socket I/O lives here.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use halcyon_world::World;
use mc_protocol::types::ItemStack;

use crate::inventory::{
    Inventory, InventoryAction, InventoryChange, InventoryError, InventorySnapshot,
};
use crate::lifecycle::{Damage, DamageKind, DamageResult, PlayerLife, PlayerRules, Teleport};
use crate::lobby::Transform;
use crate::movement::{Advance, MovementState, ProposalQueue};

const ACTIONS_PER_STEP: usize = 4;
const MAX_ACTIONS: usize = 32;

/// Arena hooks deliberately precede the first minigame implementation.
#[allow(dead_code)]
pub enum PlayerAction {
    Teleport(Transform),
    Damage(Damage),
    Heal(f32),
    Rules(PlayerRules),
    Respawn,
    Reset,
    Inventory(InventoryAction),
    EditInventory {
        edit: InventoryEdit,
        reply: glommio::channels::oneshot::Sender<Result<ItemStack, InventoryEditError>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_life_supersedes_undelivered_death_and_damage() {
        let damage = Damage { kind: DamageKind::Fall, amount: 30.0, source: None };
        let mut update = PlayerUpdate {
            health: Some(0.0),
            damage: Some(damage),
            death: Some(damage),
            ..Default::default()
        };
        update.merge(PlayerUpdate { respawn: true, health: Some(20.0), ..Default::default() });
        assert!(update.death.is_none());
        assert!(update.damage.is_none());
        assert_eq!(update.health, Some(20.0));
        assert!(update.respawn);
    }
}

/// Deliberate game-owned changes; replacing a loadout intentionally replaces its cursor too.
#[allow(dead_code)]
pub enum InventoryEdit {
    SetSlot { slot: usize, stack: ItemStack },
    Loadout { slots: Box<[ItemStack; 46]>, selected: u8 },
    Clear,
    Give(ItemStack),
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryEditError {
    #[error("player action queue is full")]
    QueueFull,
    #[error("player owner is closed")]
    Closed,
    #[error("the player life changed before the inventory edit")]
    StaleLife,
    #[error(transparent)]
    Invalid(#[from] InventoryError),
}

struct QueuedAction {
    generation: u64,
    action: PlayerAction,
}

pub struct ActionQueue {
    pending: VecDeque<QueuedAction>,
    overflow: bool,
}

impl Default for ActionQueue {
    fn default() -> Self {
        Self { pending: VecDeque::with_capacity(MAX_ACTIONS), overflow: false }
    }
}

impl ActionQueue {
    #[cfg(test)]
    pub(crate) fn capacity_payload_bytes(&self) -> usize {
        self.pending.capacity() * std::mem::size_of::<QueuedAction>()
    }
    pub fn push(&mut self, generation: u64, action: PlayerAction) -> bool {
        if self.pending.len() == MAX_ACTIONS {
            self.overflow = true;
            return false;
        }
        self.pending.push_back(QueuedAction { generation, action });
        true
    }
}

/// Replaceable self state; it never accumulates one packet per simulation step.
#[derive(Default)]
pub struct PlayerUpdate {
    pub teleport: Option<Teleport>,
    pub health: Option<f32>,
    pub damage: Option<Damage>,
    pub death: Option<Damage>,
    pub respawn: bool,
    pub inventory: Option<InventorySnapshot>,
    pub inventory_slots: u64,
    pub inventory_full: bool,
}

impl PlayerUpdate {
    pub fn merge(&mut self, newer: Self) {
        if newer.respawn {
            // A replacement life supersedes transient events from the old one.
            self.damage = None;
            self.death = None;
            self.teleport = None;
        }
        if newer.teleport.is_some() {
            self.teleport = newer.teleport;
        }
        if newer.health.is_some() {
            self.health = newer.health;
        }
        if newer.damage.is_some() {
            self.damage = newer.damage;
        }
        if newer.death.is_some() {
            self.death = newer.death;
        }
        self.respawn |= newer.respawn;
        if newer.inventory.is_some() {
            self.inventory = newer.inventory;
        }
        self.inventory_slots |= newer.inventory_slots;
        self.inventory_full |= newer.inventory_full;
    }

    pub fn changed(&self) -> bool {
        self.teleport.is_some()
            || self.health.is_some()
            || self.damage.is_some()
            || self.death.is_some()
            || self.respawn
            || self.inventory.is_some()
    }
}

pub struct PlayerState {
    pub movement: MovementState,
    pub life: PlayerLife,
    pub inventory: Inventory,
    pub equipment: Rc<[ItemStack; 6]>,
}

impl PlayerState {
    pub fn new(transform: Transform) -> Self {
        Self {
            movement: MovementState::new(transform),
            life: PlayerLife::new(transform),
            inventory: Inventory::default(),
            equipment: Rc::new(std::array::from_fn(|_| ItemStack::default())),
        }
    }

    fn teleport(
        &mut self,
        transform: Transform,
        proposals: &mut ProposalQueue,
        update: &mut PlayerUpdate,
    ) {
        if let Some(teleport) = self.life.teleport(transform) {
            self.movement.reset(transform);
            proposals.clear();
            update.teleport = Some(teleport);
        }
    }

    fn respawn(&mut self, reset: bool, proposals: &mut ProposalQueue, update: &mut PlayerUpdate) {
        if reset || self.life.respawn() {
            if reset {
                self.life.reset();
            }
            self.inventory.begin_life();
            update.health = Some(self.life.health());
            update.respawn = true;
            update.inventory = Some(self.inventory.snapshot());
            update.inventory_full = true;
            self.teleport(self.life.rules.respawn, proposals, update);
        }
    }

    fn damaged(&mut self, damage: Damage, result: DamageResult, update: &mut PlayerUpdate) {
        if let DamageResult::Hurt { health, died } = result {
            update.health = Some(health);
            update.damage = Some(damage);
            if died {
                update.death = Some(damage);
                self.inventory.cancel_drag();
            }
        }
    }

    fn inventory_changed(&mut self, changed: InventoryChange, update: &mut PlayerUpdate) {
        if changed.equipment_changed {
            self.equipment = Rc::new(self.inventory.equipment());
        }
        if changed.has_changes() {
            update.inventory_slots |= changed.changed_slots;
            update.inventory_full |= changed.full_sync;
            update.inventory = Some(self.inventory.snapshot());
        }
    }

    fn edit_inventory(
        &mut self,
        edit: InventoryEdit,
        update: &mut PlayerUpdate,
    ) -> Result<ItemStack, InventoryEditError> {
        let (remainder, changed) = match edit {
            InventoryEdit::SetSlot { slot, stack } => {
                (ItemStack::default(), self.inventory.set_slot(slot, stack)?)
            }
            InventoryEdit::Loadout { slots, selected } => {
                (ItemStack::default(), self.inventory.replace_loadout(*slots, selected)?)
            }
            InventoryEdit::Clear => (ItemStack::default(), self.inventory.reset()),
            InventoryEdit::Give(stack) => self.inventory.give(stack),
        };
        self.inventory_changed(changed, update);
        Ok(remainder)
    }

    pub fn advance(
        &mut self,
        world: &World,
        proposals: &mut ProposalQueue,
        actions: &mut ActionQueue,
        now: Instant,
        respawn_requested: bool,
    ) -> (PlayerUpdate, Option<Advance>) {
        let mut update = PlayerUpdate::default();
        if respawn_requested {
            self.respawn(false, proposals, &mut update);
        }
        if std::mem::take(&mut actions.overflow) {
            update.inventory = Some(self.inventory.snapshot());
            update.inventory_full = true;
        }
        for _ in 0..ACTIONS_PER_STEP {
            let Some(queued) = actions.pending.pop_front() else {
                break;
            };
            if queued.generation != self.life.generation {
                if let PlayerAction::EditInventory { reply, .. } = queued.action {
                    let _ = reply.send(Err(InventoryEditError::StaleLife));
                }
                continue;
            }
            match queued.action {
                PlayerAction::Teleport(transform) => {
                    self.teleport(transform, proposals, &mut update)
                }
                PlayerAction::Damage(damage) => {
                    let result = self.life.damage(damage, now);
                    self.damaged(damage, result, &mut update);
                }
                PlayerAction::Heal(amount) => {
                    if self.life.heal(amount) {
                        update.health = Some(self.life.health());
                    }
                }
                PlayerAction::Rules(rules) => {
                    if rules.respawn.is_finite() {
                        self.life.rules = rules;
                    }
                }
                PlayerAction::Respawn | PlayerAction::Reset => {
                    let reset = matches!(queued.action, PlayerAction::Reset);
                    self.respawn(reset, proposals, &mut update);
                }
                PlayerAction::Inventory(action) => {
                    if self.life.alive() {
                        let changed = self.inventory.apply(action);
                        self.inventory_changed(changed, &mut update);
                    } else {
                        update.inventory_full = true;
                        update.inventory = Some(self.inventory.snapshot());
                    }
                }
                PlayerAction::EditInventory { edit, reply } => {
                    let _ = reply.send(self.edit_inventory(edit, &mut update));
                }
            }
        }
        if !self.life.alive() || self.life.pending_teleport().is_some() {
            proposals.clear();
            return (update, None);
        }
        let movement = if proposals.is_empty() {
            None
        } else {
            Some(self.movement.advance(world, proposals))
        };
        if let Some(movement) = movement
            && movement.correction
        {
            self.teleport(movement.transform, proposals, &mut update);
        }
        if let Some(distance) = movement.and_then(|movement| movement.landed_fall_distance) {
            let result = self.life.fall_damage(distance, now);
            self.damaged(
                Damage {
                    kind: DamageKind::Fall,
                    amount: (distance - 3.0).ceil().max(0.0) as f32,
                    source: None,
                },
                result,
                &mut update,
            );
        }
        let below_world = self.movement.transform().y < f64::from(halcyon_world::MIN_BLOCK_Y - 64);
        if below_world && self.life.rules.protected {
            self.teleport(self.life.rules.respawn, proposals, &mut update);
        } else {
            let result = self.life.void_damage(below_world, now);
            self.damaged(
                Damage { kind: DamageKind::Void, amount: 4.0, source: None },
                result,
                &mut update,
            );
        }
        (update, movement)
    }
}
