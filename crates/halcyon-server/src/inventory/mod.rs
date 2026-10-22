//! Authoritative player inventory. Work is bounded by 46 menu slots; idle players
//! do no inventory work. Client predictions identify corrections, never ownership.
use mc_protocol::types::ItemStack;

mod action;
mod click;
mod slot;

pub use action::{InventoryAction, InventoryClick, MouseButton, SwapHand};
use slot::STORAGE;
pub use slot::{ClickTarget, HotbarSlot, InventorySlot};

const SLOT_COUNT: usize = InventorySlot::COUNT;
const ALL_SLOTS: u64 = (1 << SLOT_COUNT) - 1;

#[derive(Clone, Debug)]
pub struct InventorySnapshot {
    pub slots: [ItemStack; SLOT_COUNT],
    pub cursor: ItemStack,
    pub selected: HotbarSlot,
    pub state_id: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InventoryChange {
    pub changed_slots: u64,
    pub full_sync: bool,
    pub equipment_changed: bool,
    pub ignored: bool,
}

impl InventoryChange {
    pub fn has_changes(self) -> bool {
        !self.ignored
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum InventoryError {
    #[error("invalid player inventory slot")]
    InvalidSlot,
    #[error("item cannot be placed in this equipment slot")]
    RestrictedSlot,
    #[error("invalid hotbar selection")]
    InvalidSelection,
}

#[derive(Clone, Copy)]
struct Drag {
    button: MouseButton,
    slots: u64,
}

pub struct Inventory {
    slots: [ItemStack; SLOT_COUNT],
    cursor: ItemStack,
    selected: HotbarSlot,
    state_id: i32,
    drag: Option<Drag>,
    /// Loadout replacement is a barrier against clicks predicted from old items.
    require_current_state: bool,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| ItemStack::default()),
            cursor: ItemStack::default(),
            selected: HotbarSlot::FIRST,
            state_id: 0,
            drag: None,
            require_current_state: false,
        }
    }

    pub fn snapshot(&self) -> InventorySnapshot {
        InventorySnapshot {
            slots: self.slots.clone(),
            cursor: self.cursor.clone(),
            selected: self.selected,
            state_id: self.state_id,
        }
    }

    pub fn equipment(&self) -> [ItemStack; 6] {
        [
            self.slots[self.selected.inventory_slot().index()].clone(),
            self.slots[InventorySlot::OFFHAND.index()].clone(),
            self.slots[InventorySlot::FEET.index()].clone(),
            self.slots[InventorySlot::LEGS.index()].clone(),
            self.slots[InventorySlot::CHEST.index()].clone(),
            self.slots[InventorySlot::HEAD.index()].clone(),
        ]
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
    }

    pub fn begin_life(&mut self) {
        self.cancel_drag();
        self.require_current_state = true;
        self.advance_state();
    }

    fn advance_state(&mut self) {
        self.state_id = (self.state_id + 1) & 0x7fff;
    }

    fn slot_limit(slot: InventorySlot) -> u8 {
        if slot.is_armor() { 1 } else { 99 }
    }

    fn equipment_slot(stack: &ItemStack) -> Option<InventorySlot> {
        match stack.equipment_kind() {
            1 => Some(InventorySlot::FEET),
            2 => Some(InventorySlot::LEGS),
            3 => Some(InventorySlot::CHEST),
            4 => Some(InventorySlot::HEAD),
            5 => Some(InventorySlot::OFFHAND),
            _ => None,
        }
    }

    fn may_place(slot: InventorySlot, stack: &ItemStack) -> bool {
        if slot.is_crafting() {
            return false;
        }
        if stack.is_empty() || !slot.is_armor() {
            return true;
        }
        Self::equipment_slot(stack) == Some(slot)
    }

    fn validate_slot(slot: InventorySlot, stack: &ItemStack) -> Result<(), InventoryError> {
        if slot.is_crafting() {
            return Err(InventoryError::InvalidSlot);
        }
        if !Self::may_place(slot, stack) || stack.count() > Self::slot_limit(slot) {
            return Err(InventoryError::RestrictedSlot);
        }
        Ok(())
    }

    /// Gameplay mutation: a validated assignment. The caller explicitly replaces
    /// the old stack; ordinary item grants should use give and retain its remainder.
    pub fn set_slot(
        &mut self,
        slot: InventorySlot,
        stack: ItemStack,
    ) -> Result<InventoryChange, InventoryError> {
        Self::validate_slot(slot, &stack)?;
        if self.slots[slot.index()] == stack {
            return Ok(InventoryChange { ignored: true, ..Default::default() });
        }
        let before = self.equipment();
        self.cancel_drag();
        self.slots[slot.index()] = stack;
        self.advance_state();
        Ok(InventoryChange {
            changed_slots: slot.mask(),
            equipment_changed: before != self.equipment(),
            ..Default::default()
        })
    }

    /// Atomic, intentional arena reset. Validation failure leaves slots, cursor,
    /// selection and drag untouched. No old cursor survives a replacement loadout.
    pub fn replace_loadout(
        &mut self,
        slots: [ItemStack; SLOT_COUNT],
        selected: HotbarSlot,
    ) -> Result<InventoryChange, InventoryError> {
        if slots[..InventorySlot::HEAD.index()].iter().any(|item| !item.is_empty()) {
            return Err(InventoryError::InvalidSlot);
        }
        for slot in InventorySlot::all().filter(|slot| !slot.is_crafting()) {
            Self::validate_slot(slot, &slots[slot.index()])?;
        }
        let before = self.equipment();
        self.slots = slots;
        self.cursor = ItemStack::default();
        self.selected = selected;
        self.drag = None;
        self.require_current_state = true;
        self.advance_state();
        Ok(InventoryChange {
            changed_slots: ALL_SLOTS,
            full_sync: true,
            equipment_changed: before != self.equipment(),
            ignored: false,
        })
    }

    pub fn reset(&mut self) -> InventoryChange {
        self.replace_loadout(std::array::from_fn(|_| ItemStack::default()), HotbarSlot::FIRST)
            .expect("empty loadout is valid")
    }

    /// Returns any unplaced remainder rather than deleting or dropping items.
    pub fn give(&mut self, mut stack: ItemStack) -> (ItemStack, InventoryChange) {
        let before_slots = self.slots.clone();
        let before_equipment = self.equipment();
        self.insert_storage(&mut stack, &STORAGE);
        let changed_slots = changed_mask(&before_slots, &self.slots);
        if changed_slots != 0 {
            self.cancel_drag();
            self.advance_state();
        }
        (
            stack,
            InventoryChange {
                changed_slots,
                equipment_changed: before_equipment != self.equipment(),
                ignored: changed_slots == 0,
                full_sync: false,
            },
        )
    }

    pub fn apply(&mut self, action: InventoryAction) -> InventoryChange {
        if matches!(action, InventoryAction::Click { window_id,.. } | InventoryAction::Close { window_id } | InventoryAction::Reject { window_id, .. } if window_id != 0)
        {
            return InventoryChange { ignored: true, ..Default::default() };
        }
        let before_slots = self.slots.clone();
        let before_cursor = self.cursor.clone();
        let before_equipment = self.equipment();
        let before_selected = self.selected;
        let mut full_sync = false;
        let mut predicted = 0;
        let accepted = match action {
            InventoryAction::Click { state_id, click, predicted_slots, .. } => {
                predicted = predicted_slots & ALL_SLOTS;
                full_sync = state_id != self.state_id;
                if self.require_current_state && full_sync {
                    false
                } else {
                    self.require_current_state = false;
                    self.click(click)
                }
            }
            InventoryAction::Close { .. } => {
                self.cancel_drag();
                let mut cursor = std::mem::take(&mut self.cursor);
                self.insert_storage(&mut cursor, &STORAGE);
                self.cursor = cursor;
                full_sync = true;
                true
            }
            InventoryAction::Select { slot } => {
                self.selected = slot;
                true
            }
            InventoryAction::Reject { predicted_slots, .. } => {
                predicted = predicted_slots & ALL_SLOTS;
                false
            }
            InventoryAction::SwapHands => {
                self.cancel_drag();
                self.slots
                    .swap(self.selected.inventory_slot().index(), InventorySlot::OFFHAND.index());
                true
            }
            InventoryAction::RejectDrop => false,
            InventoryAction::Resync => {
                full_sync = true;
                true
            }
        };
        if !accepted {
            self.slots = before_slots.clone();
            self.cursor = before_cursor;
            self.selected = before_selected;
            self.cancel_drag();
            full_sync = true;
        }
        let changed_slots = changed_mask(&before_slots, &self.slots) | predicted;
        if changed_slots != 0 || full_sync {
            self.advance_state();
        }
        let ignored = matches!(action, InventoryAction::Select { .. })
            && before_selected == self.selected
            && !full_sync;
        InventoryChange {
            changed_slots,
            full_sync,
            equipment_changed: before_equipment != self.equipment(),
            ignored,
        }
    }

    fn insert_storage(&mut self, stack: &mut ItemStack, targets: &[InventorySlot]) {
        for empty in [false, true] {
            for &slot in targets {
                if stack.is_empty() {
                    return;
                }
                if self.slots[slot.index()].is_empty() != empty {
                    continue;
                }
                stack.transfer_to(&mut self.slots[slot.index()], u8::MAX, Self::slot_limit(slot));
            }
        }
    }
}

fn changed_mask(before: &[ItemStack; SLOT_COUNT], after: &[ItemStack; SLOT_COUNT]) -> u64 {
    InventorySlot::all()
        .zip(before.iter().zip(after))
        .fold(0, |mask, (slot, (old, new))| mask | if old != new { slot.mask() } else { 0 })
}

#[cfg(test)]
#[path = "../tests/tests_inventory.rs"]
mod tests;
