//! Authoritative player inventory. Work is bounded by 46 menu slots; idle players
//! do no inventory work. Client predictions identify corrections, never ownership.
use mc_protocol::types::ItemStack;

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

    fn click(&mut self, slot: i16, button: u8, mode: i32) -> bool {
        if mode != 5 {
            self.cancel_drag();
        }
        if mode == 5 {
            return self.drag_click(slot, button);
        }
        if slot == -999 {
            return mode == 0 && button <= 1 && self.cursor.is_empty();
        }
        let Ok(slot) = usize::try_from(slot) else {
            return false;
        };
        if !(5..SLOT_COUNT).contains(&slot) {
            return false;
        }
        match mode {
            0 if button <= 1 => self.pickup(slot, button == 1),
            1 if button <= 1 => {
                self.quick_move(slot);
                true
            }
            2 if button <= 8 || button == 40 => {
                self.swap(slot, if button == 40 { 45 } else { 36 + usize::from(button) })
            }
            6 if button <= 1 => {
                self.collect(button == 1);
                true
            }
            _ => false,
        }
    }

    fn pickup(&mut self, slot: usize, right: bool) -> bool {
        // Moving an empty bundle is ordinary storage. Its special interaction
        // with another owned stack needs nested contents, which are not enabled.
        if !self.cursor.is_empty()
            && !self.slots[slot].is_empty()
            && (self.cursor.is_bundle() || self.slots[slot].is_bundle())
        {
            return false;
        }
        if self.cursor.is_empty() {
            let count = self.slots[slot].count();
            self.cursor = self.slots[slot].take(if right { count.div_ceil(2) } else { count });
            return true;
        }
        if !Self::may_place(slot, &self.cursor) {
            return false;
        }
        if self.slots[slot].is_empty() || self.cursor.same_item(&self.slots[slot]) {
            let amount = if right { 1 } else { self.cursor.count() };
            self.cursor.transfer_to(&mut self.slots[slot], amount, Self::slot_limit(slot));
            true
        } else if self.cursor.count() <= Self::slot_limit(slot) {
            std::mem::swap(&mut self.cursor, &mut self.slots[slot]);
            true
        } else {
            false
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

    fn quick_move(&mut self, slot: usize) {
        let mut source = std::mem::take(&mut self.slots[slot]);
        if source.is_empty() {
            return;
        }
        if (9..45).contains(&slot) {
            let kind = source.equipment_kind();
            let equipment = match kind {
                1..=4 => Some(9 - usize::from(kind)),
                5 => Some(45),
                _ => None,
            };
            if let Some(target) = equipment.filter(|&target| self.slots[target].is_empty()) {
                source.transfer_to(&mut self.slots[target], 1, Self::slot_limit(target));
                self.slots[slot] = source;
                return;
            }
            if slot < 36 {
                self.insert_storage(&mut source, &STORAGE[..9]);
            } else {
                self.insert_storage(&mut source, &STORAGE[9..]);
            }
        } else {
            self.insert_storage(&mut source, &STORAGE);
        }
        self.slots[slot] = source;
    }

    fn swap(&mut self, clicked: usize, hand: usize) -> bool {
        if clicked == hand {
            return true;
        }
        if !Self::may_place(clicked, &self.slots[hand])
            || !Self::may_place(hand, &self.slots[clicked])
        {
            return false;
        }
        let limit = Self::slot_limit(clicked);
        if self.slots[hand].count() <= limit {
            self.slots.swap(clicked, hand);
            return true;
        }
        let mut displaced = std::mem::take(&mut self.slots[clicked]);
        self.slots[clicked] = self.slots[hand].take(limit);
        self.insert_storage(&mut displaced, &STORAGE);
        displaced.is_empty() // apply rolls the whole action back if relocation cannot fit.
    }

    fn drag_click(&mut self, slot: i16, button: u8) -> bool {
        let (right, stage) = match button {
            0..=2 => (false, button),
            4..=6 => (true, button - 4),
            _ => return false,
        };
        match stage {
            0 if slot == -999 && !self.cursor.is_empty() => {
                if self.drag.is_some() {
                    self.drag = None;
                    return false;
                }
                self.drag = Some(Drag { right, slots: 0 });
                true
            }
            1 => {
                let Some(mut drag) = self.drag else {
                    return false;
                };
                let Ok(slot) = usize::try_from(slot) else {
                    return false;
                };
                if drag.right != right || !Self::may_place(slot, &self.cursor) {
                    return false;
                }
                let target = &self.slots[slot];
                if (target.is_empty() || target.same_item(&self.cursor))
                    && target.count() < self.cursor.max_stack().min(Self::slot_limit(slot))
                    && (drag.slots & (1 << slot) != 0
                        || drag.slots.count_ones() < u32::from(self.cursor.count()))
                {
                    drag.slots |= 1 << slot;
                }
                self.drag = Some(drag);
                true
            }
            2 if slot == -999 => {
                let Some(drag) = self.drag.take() else {
                    return false;
                };
                if drag.right != right {
                    return false;
                }
                if drag.slots == 0 {
                    return true;
                }
                let amount = if right {
                    1
                } else {
                    self.cursor.count() / drag.slots.count_ones() as u8
                };
                for slot in 5..SLOT_COUNT {
                    if drag.slots & (1 << slot) != 0 {
                        self.cursor.transfer_to(
                            &mut self.slots[slot],
                            amount,
                            Self::slot_limit(slot),
                        );
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn collect(&mut self, reverse: bool) {
        if self.cursor.is_empty() {
            return;
        }
        for partial_only in [true, false] {
            for index in 5..SLOT_COUNT {
                let slot = if reverse { SLOT_COUNT + 4 - index } else { index };
                if !self.cursor.same_item(&self.slots[slot])
                    || (partial_only && self.slots[slot].count() == self.slots[slot].max_stack())
                {
                    continue;
                }
                self.slots[slot].transfer_to(&mut self.cursor, u8::MAX, 99);
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
mod tests {
    use super::*;
    use mc_protocol::data::items::ITEMS;
    use mc_protocol::types::ItemComponents;

    fn item(name: &str, count: u8) -> ItemStack {
        let id = ITEMS.iter().position(|item| item.0 == name).unwrap() as i32;
        ItemStack::plain(id, count).unwrap()
    }

    fn click(inventory: &mut Inventory, slot: i16, button: u8, mode: i32) -> InventoryChange {
        inventory.apply(InventoryAction::Click {
            window_id: 0,
            state_id: inventory.state_id,
            slot,
            button,
            mode,
            predicted_slots: 0,
        })
    }

    fn total(inventory: &Inventory) -> usize {
        inventory.slots.iter().map(|stack| usize::from(stack.count())).sum::<usize>()
            + usize::from(inventory.cursor.count())
    }

    #[test]
    fn rearrangements_conserve_items_and_corrections_never_grant_predictions() {
        let mut inventory = Inventory::new();
        inventory.set_slot(9, item("minecraft:stone", 11)).unwrap();
        inventory.set_slot(10, item("minecraft:stone", 60)).unwrap();
        click(&mut inventory, 9, 1, 0);
        assert_eq!(inventory.cursor.count(), 6);
        click(&mut inventory, 10, 0, 0);
        assert_eq!(inventory.cursor.count(), 2);
        click(&mut inventory, 11, 1, 0);
        click(&mut inventory, 12, 0, 0);
        assert_eq!(total(&inventory), 71);
        click(&mut inventory, 9, 0, 1);
        assert_eq!(inventory.slots[36].count(), 5);
        click(&mut inventory, 36, 40, 2);
        assert_eq!(inventory.slots[45].count(), 5);
        inventory.apply(InventoryAction::SwapHands);
        assert_eq!(inventory.equipment()[0].count(), 5);
        let correction = inventory.apply(InventoryAction::Click {
            window_id: 0,
            state_id: -1,
            slot: 13,
            button: 0,
            mode: 0,
            predicted_slots: 1 << 44,
        });
        assert!(correction.full_sync);
        assert_ne!(correction.changed_slots & (1 << 44), 0);
        assert!(inventory.slots[44].is_empty());
        assert_eq!(total(&inventory), 71);
        for action in [
            InventoryAction::RejectDrop,
            InventoryAction::Click {
                window_id: 0,
                state_id: inventory.state_id,
                slot: 36,
                button: 0,
                mode: 4,
                predicted_slots: 0,
            },
            InventoryAction::Click {
                window_id: 0,
                state_id: inventory.state_id,
                slot: 36,
                button: 2,
                mode: 3,
                predicted_slots: 0,
            },
        ] {
            assert!(inventory.apply(action).full_sync);
            assert_eq!(total(&inventory), 71);
        }
    }

    #[test]
    fn equipment_slots_restrict_items_and_number_swaps_relocate_without_loss() {
        let mut inventory = Inventory::new();
        assert_eq!(
            inventory.set_slot(5, item("minecraft:stone", 1)).unwrap_err(),
            InventoryError::RestrictedSlot
        );
        inventory.set_slot(9, item("minecraft:diamond_helmet", 1)).unwrap();
        click(&mut inventory, 9, 0, 1);
        assert_eq!(inventory.equipment()[5], item("minecraft:diamond_helmet", 1));
        inventory.set_slot(36, item("minecraft:carved_pumpkin", 64)).unwrap();
        assert!(!click(&mut inventory, 5, 0, 2).full_sync);
        assert_eq!(inventory.slots[5].count(), 1);
        assert_eq!(inventory.slots[36].count(), 63);
        assert_eq!(total(&inventory), 65);
        assert!(inventory.slots[37].same_item(&item("minecraft:diamond_helmet", 1)));
        assert!(inventory.apply(InventoryAction::Select { slot: 9 }).full_sync);
        assert_eq!(inventory.selected, 0);
        assert!(inventory.apply(InventoryAction::Select { slot: 1 }).equipment_changed);
    }

    #[test]
    fn drag_collect_and_close_preserve_components_and_full_storage_cursor() {
        let mut inventory = Inventory::new();
        inventory.set_slot(9, item("minecraft:stone", 11)).unwrap();
        click(&mut inventory, 9, 0, 0);
        click(&mut inventory, -999, 0, 5);
        click(&mut inventory, 9, 1, 5);
        click(&mut inventory, 10, 1, 5);
        click(&mut inventory, 11, 1, 5);
        click(&mut inventory, -999, 2, 5);
        assert_eq!(
            [inventory.slots[9].count(), inventory.slots[10].count(), inventory.slots[11].count()],
            [3, 3, 3]
        );
        assert_eq!(inventory.cursor.count(), 2);
        click(&mut inventory, 9, 0, 6);
        assert_eq!(inventory.cursor.count(), 11);
        assert!(inventory.slots[9].is_empty());
        inventory.apply(InventoryAction::Close { window_id: 0 });
        assert!(inventory.cursor.is_empty());
        assert_eq!(total(&inventory), 11);

        inventory.set_slot(9, item("minecraft:bundle", 1)).unwrap();
        click(&mut inventory, 36, 0, 0);
        assert!(click(&mut inventory, 9, 1, 0).full_sync);
        assert_eq!(total(&inventory), 12);
        inventory.apply(InventoryAction::Close { window_id: 0 });

        for slot in STORAGE {
            inventory.set_slot(slot, item("minecraft:stone", 64)).unwrap();
        }
        inventory.set_slot(5, item("minecraft:diamond_helmet", 1)).unwrap();
        click(&mut inventory, 5, 0, 0);
        let before = total(&inventory);
        inventory.apply(InventoryAction::Close { window_id: 0 });
        assert_eq!(inventory.cursor, item("minecraft:diamond_helmet", 1));
        assert_eq!(total(&inventory), before);
        assert!(click(&mut inventory, -999, 0, 0).full_sync);
        assert_eq!(total(&inventory), before);
        let id = item("minecraft:stone", 1).item_id();
        let named = ItemStack::new(
            id,
            1,
            ItemComponents { name: Some("Distinct".into()), ..Default::default() },
        )
        .unwrap();
        let (remainder, change) = inventory.give(named.clone());
        assert_eq!(remainder, named);
        assert!(change.ignored);
    }

    #[test]
    fn replacement_is_atomic_clears_cursor_and_blocks_old_loadout_clicks() {
        let mut inventory = Inventory::new();
        inventory.set_slot(36, item("minecraft:stone", 5)).unwrap();
        click(&mut inventory, 36, 0, 0);
        click(&mut inventory, -999, 0, 5);
        inventory.cancel_drag(); // death retains ownership while cancelling gestures.
        assert_eq!(total(&inventory), 5);
        let mut invalid = std::array::from_fn(|_| ItemStack::default());
        invalid[5] = item("minecraft:stone", 1);
        assert!(inventory.replace_loadout(invalid, 0).is_err());
        assert_eq!(inventory.cursor.count(), 5);
        let old_state = inventory.state_id;
        inventory.reset();
        assert_eq!(total(&inventory), 0);
        inventory.set_slot(36, item("minecraft:stone", 7)).unwrap();
        let change = inventory.apply(InventoryAction::Click {
            window_id: 0,
            state_id: old_state,
            slot: 36,
            button: 0,
            mode: 0,
            predicted_slots: 0,
        });
        assert!(change.full_sync);
        assert!(inventory.cursor.is_empty());
        assert_eq!(inventory.slots[36].count(), 7);
        click(&mut inventory, 36, 0, 0);
        assert_eq!(inventory.cursor.count(), 7);
    }
}
