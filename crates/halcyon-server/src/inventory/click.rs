//! Validated menu click operations; the inventory owner handles rollback and synchronization.

use super::{
    ClickTarget, Drag, HotbarSlot, Inventory, InventoryClick, InventorySlot, MouseButton, STORAGE,
    SwapHand,
};

impl Inventory {
    pub(super) fn click(&mut self, click: InventoryClick) -> bool {
        if !matches!(
            click,
            InventoryClick::DragStart { .. }
                | InventoryClick::DragAdd { .. }
                | InventoryClick::DragFinish { .. }
        ) {
            self.cancel_drag();
        }
        match click {
            InventoryClick::Pickup { target: ClickTarget::Outside, .. } => self.cursor.is_empty(),
            InventoryClick::Pickup { target: ClickTarget::Slot(slot), button } => {
                !slot.is_crafting() && self.pickup(slot, button)
            }
            InventoryClick::QuickMove { slot } => {
                if slot.is_crafting() {
                    return false;
                }
                self.quick_move(slot);
                true
            }
            InventoryClick::Swap { slot, hand } => {
                if slot.is_crafting() {
                    return false;
                }
                let hand = match hand {
                    SwapHand::Hotbar(slot) => slot.inventory_slot(),
                    SwapHand::Offhand => InventorySlot::OFFHAND,
                };
                self.swap(slot, hand)
            }
            InventoryClick::DragStart { button } => self.start_drag(button),
            InventoryClick::DragAdd { slot, button } => self.add_drag_slot(slot, button),
            InventoryClick::DragFinish { button } => self.finish_drag(button),
            InventoryClick::Collect { slot, button } => {
                if slot.is_crafting() {
                    return false;
                }
                self.collect(button);
                true
            }
        }
    }

    fn pickup(&mut self, slot: InventorySlot, button: MouseButton) -> bool {
        // Moving an empty bundle is ordinary storage. Its special interaction
        // with another owned stack needs nested contents, which are not enabled.
        if !self.cursor.is_empty()
            && !self.slots[slot.index()].is_empty()
            && (self.cursor.is_bundle() || self.slots[slot.index()].is_bundle())
        {
            return false;
        }
        if self.cursor.is_empty() {
            let count = self.slots[slot.index()].count();
            self.cursor = self.slots[slot.index()].take(if button == MouseButton::Right {
                count.div_ceil(2)
            } else {
                count
            });
            return true;
        }
        if !Self::may_place(slot, &self.cursor) {
            return false;
        }
        if self.slots[slot.index()].is_empty() || self.cursor.same_item(&self.slots[slot.index()]) {
            let amount = if button == MouseButton::Right {
                1
            } else {
                self.cursor.count()
            };
            self.cursor.transfer_to(&mut self.slots[slot.index()], amount, Self::slot_limit(slot));
            true
        } else if self.cursor.count() <= Self::slot_limit(slot) {
            std::mem::swap(&mut self.cursor, &mut self.slots[slot.index()]);
            true
        } else {
            false
        }
    }

    fn quick_move(&mut self, slot: InventorySlot) {
        let mut source = std::mem::take(&mut self.slots[slot.index()]);
        if source.is_empty() {
            return;
        }
        if slot.is_storage() {
            let equipment = Self::equipment_slot(&source);
            if let Some(target) = equipment.filter(|&target| self.slots[target.index()].is_empty())
            {
                source.transfer_to(&mut self.slots[target.index()], 1, Self::slot_limit(target));
                self.slots[slot.index()] = source;
                return;
            }
            if slot.hotbar_position().is_none() {
                self.insert_storage(&mut source, &STORAGE[..HotbarSlot::COUNT]);
            } else {
                self.insert_storage(&mut source, &STORAGE[HotbarSlot::COUNT..]);
            }
        } else {
            self.insert_storage(&mut source, &STORAGE);
        }
        self.slots[slot.index()] = source;
    }

    fn swap(&mut self, clicked: InventorySlot, hand: InventorySlot) -> bool {
        if clicked == hand {
            return true;
        }
        if !Self::may_place(clicked, &self.slots[hand.index()])
            || !Self::may_place(hand, &self.slots[clicked.index()])
        {
            return false;
        }
        let limit = Self::slot_limit(clicked);
        if self.slots[hand.index()].count() <= limit {
            self.slots.swap(clicked.index(), hand.index());
            return true;
        }
        let mut displaced = std::mem::take(&mut self.slots[clicked.index()]);
        self.slots[clicked.index()] = self.slots[hand.index()].take(limit);
        self.insert_storage(&mut displaced, &STORAGE);
        displaced.is_empty() // apply rolls the whole action back if relocation cannot fit.
    }

    fn start_drag(&mut self, button: MouseButton) -> bool {
        if self.cursor.is_empty() || self.drag.is_some() {
            return false;
        }
        self.drag = Some(Drag { button, slots: 0 });
        true
    }

    fn add_drag_slot(&mut self, slot: InventorySlot, button: MouseButton) -> bool {
        let Some(mut drag) = self.drag else {
            return false;
        };
        if drag.button != button || !Self::may_place(slot, &self.cursor) {
            return false;
        }
        let target = &self.slots[slot.index()];
        if (target.is_empty() || target.same_item(&self.cursor))
            && target.count() < self.cursor.max_stack().min(Self::slot_limit(slot))
            && (drag.slots & slot.mask() != 0
                || drag.slots.count_ones() < u32::from(self.cursor.count()))
        {
            drag.slots |= slot.mask();
        }
        self.drag = Some(drag);
        true
    }

    fn finish_drag(&mut self, button: MouseButton) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        if drag.button != button {
            return false;
        }
        if drag.slots == 0 {
            return true;
        }
        let amount = match button {
            MouseButton::Right => 1,
            MouseButton::Left => self.cursor.count() / drag.slots.count_ones() as u8,
        };
        for slot in InventorySlot::all().filter(|slot| !slot.is_crafting()) {
            if drag.slots & slot.mask() != 0 {
                self.cursor.transfer_to(
                    &mut self.slots[slot.index()],
                    amount,
                    Self::slot_limit(slot),
                );
            }
        }
        true
    }

    fn collect(&mut self, button: MouseButton) {
        if self.cursor.is_empty() {
            return;
        }
        for partial_only in [true, false] {
            let mut slots = InventorySlot::all().filter(|slot| !slot.is_crafting());
            while let Some(slot) = if button == MouseButton::Right {
                slots.next_back()
            } else {
                slots.next()
            } {
                if !self.cursor.same_item(&self.slots[slot.index()])
                    || (partial_only
                        && self.slots[slot.index()].count() == self.slots[slot.index()].max_stack())
                {
                    continue;
                }
                self.slots[slot.index()].transfer_to(&mut self.cursor, u8::MAX, 99);
            }
        }
    }
}
