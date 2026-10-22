//! Checked menu identities. Access and item-placement policy belong to the inventory.

use super::InventoryError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InventorySlot(u8);

impl InventorySlot {
    pub const COUNT: usize = 46;
    pub const HEAD: Self = Self(5);
    pub const CHEST: Self = Self(6);
    pub const LEGS: Self = Self(7);
    pub const FEET: Self = Self(8);
    pub const OFFHAND: Self = Self(45);

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub const fn protocol_id(self) -> i16 {
        self.0 as i16
    }

    pub const fn mask(self) -> u64 {
        1 << self.0
    }

    pub const fn is_crafting(self) -> bool {
        self.0 < 5
    }

    pub const fn is_armor(self) -> bool {
        self.0 >= 5 && self.0 < 9
    }

    pub const fn is_storage(self) -> bool {
        self.0 >= 9 && self.0 < 45
    }

    pub const fn hotbar_position(self) -> Option<HotbarSlot> {
        if self.0 >= 36 && self.0 < 45 {
            Some(HotbarSlot(self.0 - 36))
        } else {
            None
        }
    }

    pub fn all() -> impl DoubleEndedIterator<Item = Self> + ExactSizeIterator {
        (0..Self::COUNT as u8).map(Self)
    }
}

impl TryFrom<i16> for InventorySlot {
    type Error = InventoryError;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        if (0..Self::COUNT as i16).contains(&value) {
            Ok(Self(value as u8))
        } else {
            Err(InventoryError::InvalidSlot)
        }
    }
}

impl TryFrom<usize> for InventorySlot {
    type Error = InventoryError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value < Self::COUNT {
            Ok(Self(value as u8))
        } else {
            Err(InventoryError::InvalidSlot)
        }
    }
}

/// A held-item selection, distinct from its menu slot (36 through 44).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HotbarSlot(u8);

impl HotbarSlot {
    pub const COUNT: usize = 9;
    pub const FIRST: Self = Self(0);

    pub const fn protocol_id(self) -> u8 {
        self.0
    }

    pub const fn inventory_slot(self) -> InventorySlot {
        InventorySlot(36 + self.0)
    }
}

impl TryFrom<i16> for HotbarSlot {
    type Error = InventoryError;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        if (0..Self::COUNT as i16).contains(&value) {
            Ok(Self(value as u8))
        } else {
            Err(InventoryError::InvalidSelection)
        }
    }
}

impl TryFrom<usize> for HotbarSlot {
    type Error = InventoryError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value < Self::COUNT {
            Ok(Self(value as u8))
        } else {
            Err(InventoryError::InvalidSelection)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickTarget {
    Slot(InventorySlot),
    Outside,
}

impl TryFrom<i16> for ClickTarget {
    type Error = InventoryError;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        if value == -999 {
            Ok(Self::Outside)
        } else {
            InventorySlot::try_from(value).map(Self::Slot)
        }
    }
}

/// Preserve hotbar-first insertion order without constructing unchecked indices at call sites.
pub(super) const STORAGE: [InventorySlot; 36] = {
    let mut slots = [InventorySlot(0); 36];
    let mut index = 0;
    while index < slots.len() {
        slots[index] = InventorySlot(if index < HotbarSlot::COUNT {
            36 + index as u8
        } else {
            index as u8
        });
        index += 1;
    }
    slots
};

#[cfg(test)]
#[path = "../tests/tests_inventory_slots.rs"]
mod tests;
