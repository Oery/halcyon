//! Convert untrusted menu coordinates before they enter the gameplay queue.

use super::{ClickTarget, HotbarSlot, InventorySlot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

impl MouseButton {
    fn from_raw(button: u8) -> Option<Self> {
        match button {
            0 => Some(Self::Left),
            1 => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapHand {
    Hotbar(HotbarSlot),
    Offhand,
}

/// Structurally valid menu operations. The inventory owner still validates
/// placement, crafting restrictions, cursor ownership and drag gesture state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryClick {
    Pickup { target: ClickTarget, button: MouseButton },
    QuickMove { slot: InventorySlot },
    Swap { slot: InventorySlot, hand: SwapHand },
    DragStart { button: MouseButton },
    DragAdd { slot: InventorySlot, button: MouseButton },
    DragFinish { button: MouseButton },
    Collect { slot: InventorySlot, button: MouseButton },
}

impl InventoryClick {
    fn from_raw(slot: i16, button: u8, mode: i32) -> Option<Self> {
        let target = ClickTarget::try_from(slot).ok()?;
        match (mode, target) {
            (0, target) => Some(Self::Pickup { target, button: MouseButton::from_raw(button)? }),
            (1, ClickTarget::Slot(slot)) if button <= 1 => Some(Self::QuickMove { slot }),
            (2, ClickTarget::Slot(slot)) => {
                let hand = if button == 40 {
                    SwapHand::Offhand
                } else {
                    SwapHand::Hotbar(HotbarSlot::try_from(i16::from(button)).ok()?)
                };
                Some(Self::Swap { slot, hand })
            }
            (5, target) => match (button, target) {
                (0, ClickTarget::Outside) => Some(Self::DragStart { button: MouseButton::Left }),
                (4, ClickTarget::Outside) => Some(Self::DragStart { button: MouseButton::Right }),
                (1, ClickTarget::Slot(slot)) => {
                    Some(Self::DragAdd { slot, button: MouseButton::Left })
                }
                (5, ClickTarget::Slot(slot)) => {
                    Some(Self::DragAdd { slot, button: MouseButton::Right })
                }
                (2, ClickTarget::Outside) => Some(Self::DragFinish { button: MouseButton::Left }),
                (6, ClickTarget::Outside) => Some(Self::DragFinish { button: MouseButton::Right }),
                _ => None,
            },
            (6, ClickTarget::Slot(slot)) => {
                Some(Self::Collect { slot, button: MouseButton::from_raw(button)? })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum InventoryAction {
    Click {
        window_id: i32,
        state_id: i32,
        click: InventoryClick,
        predicted_slots: u64,
    },
    Close {
        window_id: i32,
    },
    Select {
        slot: HotbarSlot,
    },
    /// Reject an invalid prediction while retaining its window and correction mask.
    Reject {
        window_id: i32,
        predicted_slots: u64,
    },
    SwapHands,
    RejectDrop,
    Resync,
}

impl InventoryAction {
    pub fn from_click(
        window_id: i32,
        state_id: i32,
        slot: i16,
        button: u8,
        mode: i32,
        predicted_slots: u64,
    ) -> Self {
        match InventoryClick::from_raw(slot, button, mode) {
            Some(click) => Self::Click { window_id, state_id, click, predicted_slots },
            None => Self::Reject { window_id, predicted_slots },
        }
    }

    pub fn from_selection(slot: i16) -> Self {
        match HotbarSlot::try_from(slot) {
            Ok(slot) => Self::Select { slot },
            Err(_) => Self::Reject { window_id: 0, predicted_slots: 0 },
        }
    }
}

#[cfg(test)]
#[path = "../tests/tests_inventory_clicks.rs"]
mod tests;
