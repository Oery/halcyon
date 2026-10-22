//! Convert untrusted menu coordinates before they enter the gameplay queue.

use super::{ClickTarget, HotbarSlot};

#[derive(Clone, Copy, Debug)]
pub enum InventoryAction {
    Click {
        window_id: i32,
        state_id: i32,
        target: ClickTarget,
        button: u8,
        mode: i32,
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
        match ClickTarget::try_from(slot) {
            Ok(target) => {
                Self::Click { window_id, state_id, target, button, mode, predicted_slots }
            }
            Err(_) => Self::Reject { window_id, predicted_slots },
        }
    }

    pub fn from_selection(slot: i16) -> Self {
        match HotbarSlot::try_from(slot) {
            Ok(slot) => Self::Select { slot },
            Err(_) => Self::Reject { window_id: 0, predicted_slots: 0 },
        }
    }
}
