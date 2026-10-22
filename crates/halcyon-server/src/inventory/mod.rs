//! Authoritative player inventory. Work is bounded by 46 menu slots; idle players
//! do no inventory work. Client predictions identify corrections, never ownership.
use mc_protocol::types::ItemStack;

mod click;

const SLOT_COUNT: usize = 46;
const ALL_SLOTS: u64 = (1 << SLOT_COUNT) - 1;
const STORAGE: [usize; 36] = [
    36, 37, 38, 39, 40, 41, 42, 43, 44, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
];

#[derive(Clone, Debug)]
pub struct InventorySnapshot {
    pub slots: [ItemStack; SLOT_COUNT],
    pub cursor: ItemStack,
    pub selected: u8,
    pub state_id: i32,
}

#[derive(Clone, Copy, Debug)]
pub enum InventoryAction {
    Click { window_id: i32, state_id: i32, slot: i16, button: u8, mode: i32, predicted_slots: u64 },
    Close { window_id: i32 },
    Select { slot: i16 },
    SwapHands,
    RejectDrop,
    Resync,
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
    right: bool,
    slots: u64,
}

pub struct Inventory {
    slots: [ItemStack; SLOT_COUNT],
    cursor: ItemStack,
    selected: u8,
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
            selected: 0,
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
            self.slots[36 + usize::from(self.selected)].clone(),
            self.slots[45].clone(),
            self.slots[8].clone(),
            self.slots[7].clone(),
            self.slots[6].clone(),
            self.slots[5].clone(),
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

    fn slot_limit(slot: usize) -> u8 {
        if (5..9).contains(&slot) { 1 } else { 99 }
    }

    fn may_place(slot: usize, stack: &ItemStack) -> bool {
        if !(5..SLOT_COUNT).contains(&slot) {
            return false;
        }
        if stack.is_empty() || slot >= 9 {
            return true;
        }
        usize::from(stack.equipment_kind()) == 9 - slot
    }

    fn validate_slot(slot: usize, stack: &ItemStack) -> Result<(), InventoryError> {
        if !(5..SLOT_COUNT).contains(&slot) {
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
        slot: usize,
        stack: ItemStack,
    ) -> Result<InventoryChange, InventoryError> {
        Self::validate_slot(slot, &stack)?;
        if self.slots[slot] == stack {
            return Ok(InventoryChange { ignored: true, ..Default::default() });
        }
        let before = self.equipment();
        self.cancel_drag();
        self.slots[slot] = stack;
        self.advance_state();
        Ok(InventoryChange {
            changed_slots: 1 << slot,
            equipment_changed: before != self.equipment(),
            ..Default::default()
        })
    }

    /// Atomic, intentional arena reset. Validation failure leaves slots, cursor,
    /// selection and drag untouched. No old cursor survives a replacement loadout.
    pub fn replace_loadout(
        &mut self,
        slots: [ItemStack; SLOT_COUNT],
        selected: u8,
    ) -> Result<InventoryChange, InventoryError> {
        if selected > 8 {
            return Err(InventoryError::InvalidSelection);
        }
        if slots[..5].iter().any(|item| !item.is_empty()) {
            return Err(InventoryError::InvalidSlot);
        }
        for (slot, stack) in slots.iter().enumerate().skip(5) {
            Self::validate_slot(slot, stack)?;
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
        self.replace_loadout(std::array::from_fn(|_| ItemStack::default()), 0)
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
        if matches!(action, InventoryAction::Click { window_id,.. } | InventoryAction::Close { window_id } if window_id != 0)
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
            InventoryAction::Click { state_id, slot, button, mode, predicted_slots, .. } => {
                predicted = predicted_slots & ALL_SLOTS;
                full_sync = state_id != self.state_id;
                if self.require_current_state && full_sync {
                    false
                } else {
                    self.require_current_state = false;
                    self.click(slot, button, mode)
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
                if (0..9).contains(&slot) {
                    self.selected = slot as u8;
                    true
                } else {
                    false
                }
            }
            InventoryAction::SwapHands => {
                self.cancel_drag();
                self.slots.swap(36 + usize::from(self.selected), 45);
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

    fn insert_storage(&mut self, stack: &mut ItemStack, targets: &[usize]) {
        for empty in [false, true] {
            for &slot in targets {
                if stack.is_empty() {
                    return;
                }
                if self.slots[slot].is_empty() != empty {
                    continue;
                }
                stack.transfer_to(&mut self.slots[slot], u8::MAX, Self::slot_limit(slot));
            }
        }
    }
}

fn changed_mask(before: &[ItemStack; SLOT_COUNT], after: &[ItemStack; SLOT_COUNT]) -> u64 {
    before
        .iter()
        .zip(after)
        .enumerate()
        .fold(0, |mask, (slot, (old, new))| mask | if old != new { 1 << slot } else { 0 })
}

#[cfg(test)]
#[path = "../tests/tests_inventory.rs"]
mod tests;
