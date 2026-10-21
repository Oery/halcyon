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

    fn empty_world() -> World {
        World::from_loaded(halcyon_world::LoadedWorld {
            chunks: Vec::new(),
            spawn: (0.0, 0.0, 0.0, 0.0, 0.0),
            missing_blocks: 0,
            remapped_biomes: 0,
        })
    }

    #[test]
    fn a_new_life_supersedes_undelivered_death_and_damage() {
        let damage = Damage { kind: DamageKind::Fall, amount: 30.0, source: None };
        let mut player = PlayerState::new(Transform::default());
        player.life.rules.protected = false;
        let mut actions = ActionQueue::default();
        actions.push(0, PlayerAction::Damage(damage));
        actions.push(0, PlayerAction::Reset);
        actions.push(0, PlayerAction::Damage(damage)); // Old-life work after reset is stale.
        let (newer, _) = player.advance(
            &empty_world(),
            &mut ProposalQueue::default(),
            &mut actions,
            Instant::now(),
            false,
        );
        assert!(player.life.alive());
        assert_eq!(player.life.health(), 20.0);
        assert_eq!(player.life.generation, 1);
        let mut update = PlayerUpdate {
            health: Some(0.0),
            damage: Some(damage),
            death: Some(damage),
            ..Default::default()
        };
        update.merge(newer);
        assert!(update.death.is_none());
        assert!(update.damage.is_none());
        assert_eq!(update.health, Some(20.0));
        assert!(update.respawn);
    }

    #[test]
    fn invalid_destinations_preserve_position_teleport_and_rules() {
        let spawn = Transform::default();
        let mut player = PlayerState::new(spawn);
        let pending = player.life.teleport(spawn).unwrap();
        for destination in [
            Transform { x: 30_000_000.1, ..spawn },
            Transform { y: -20_000_000.1, ..spawn },
            Transform { z: -30_000_000.1, ..spawn },
            Transform { x: f64::NAN, ..spawn },
            Transform { pitch: f32::INFINITY, ..spawn },
        ] {
            assert!(player.life.teleport(destination).is_none());
            let mut actions = ActionQueue::default();
            actions.push(0, PlayerAction::Teleport(destination));
            actions.push(
                0,
                PlayerAction::Rules(PlayerRules {
                    protected: false,
                    respawn: destination,
                    ..player.life.rules
                }),
            );
            let (update, _) = player.advance(
                &empty_world(),
                &mut ProposalQueue::default(),
                &mut actions,
                Instant::now(),
                false,
            );
            assert!(!update.changed());
            assert_eq!(player.movement.transform(), spawn);
            assert_eq!(player.life.pending_teleport(), Some(pending));
            assert_eq!(player.life.rules.respawn, spawn);
            assert!(player.life.rules.protected);
        }
        for sign in [-1.0, 1.0] {
            let boundary = Transform {
                x: sign * 30_000_000.0,
                y: sign * 20_000_000.0,
                z: sign * 30_000_000.0,
                ..spawn
            };
            assert!(player.life.teleport(boundary).is_some());
        }
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
    fn clear_life_events(&mut self) {
        self.damage = None;
        self.death = None;
        self.teleport = None;
    }

    pub fn merge(&mut self, newer: Self) {
        if newer.respawn {
            // A replacement life supersedes transient events from the old one.
            self.clear_life_events();
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
            update.clear_life_events();
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
                    if rules.respawn.is_valid() {
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
