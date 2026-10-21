//! Validated menu click operations; the inventory owner handles rollback and synchronization.

use super::{Drag, Inventory, SLOT_COUNT, STORAGE};

impl Inventory {
    pub(super) fn click(&mut self, slot: i16, button: u8, mode: i32) -> bool {
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
