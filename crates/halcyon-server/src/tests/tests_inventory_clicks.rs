use super::*;

#[test]
fn wire_clicks_decode_operations_with_only_their_valid_arguments() {
    use InventoryClick::*;
    use MouseButton::{Left, Right};

    let slot = InventorySlot::try_from(9usize).unwrap();
    let target = ClickTarget::Slot(slot);
    for (raw, button, mode, expected) in [
        (9, 0, 0, Pickup { target, button: Left }),
        (9, 1, 0, Pickup { target, button: Right }),
        (-999, 0, 0, Pickup { target: ClickTarget::Outside, button: Left }),
        (-999, 1, 0, Pickup { target: ClickTarget::Outside, button: Right }),
        (9, 0, 1, QuickMove { slot }),
        (9, 1, 1, QuickMove { slot }),
        (9, 0, 2, Swap { slot, hand: SwapHand::Hotbar(HotbarSlot::FIRST) }),
        (9, 8, 2, Swap { slot, hand: SwapHand::Hotbar(HotbarSlot::try_from(8usize).unwrap()) }),
        (9, 40, 2, Swap { slot, hand: SwapHand::Offhand }),
        (-999, 0, 5, DragStart { button: Left }),
        (-999, 4, 5, DragStart { button: Right }),
        (9, 1, 5, DragAdd { slot, button: Left }),
        (9, 5, 5, DragAdd { slot, button: Right }),
        (-999, 2, 5, DragFinish { button: Left }),
        (-999, 6, 5, DragFinish { button: Right }),
        (9, 0, 6, Collect { slot, button: Left }),
        (9, 1, 6, Collect { slot, button: Right }),
    ] {
        let InventoryAction::Click { click, window_id, state_id, predicted_slots } =
            InventoryAction::from_click(0, 17, raw, button, mode, 1 << 44)
        else {
            panic!("rejected supported click: {raw}, {button}, {mode}");
        };
        assert_eq!(click, expected);
        assert_eq!((window_id, state_id, predicted_slots), (0, 17, 1 << 44));
    }
    // Crafting identities are valid coordinates; access remains an owner policy.
    assert!(InventoryClick::from_raw(0, 0, 0).is_some());
    for (slot, button, mode) in [
        (9, 2, 0),
        (9, 2, 1),
        (9, 9, 2),
        (9, 39, 2),
        (9, 41, 2),
        (9, 2, 6),
        (9, 0, 3),
        (9, 0, 4),
        (9, 0, -1),
        (9, 0, i32::MAX),
        (-999, 0, 1),
        (-999, 0, 2),
        (-999, 0, 6),
        (9, 0, 5),
        (9, 2, 5),
        (9, 4, 5),
        (9, 6, 5),
        (-999, 1, 5),
        (-999, 5, 5),
        (-999, 8, 5),
        (-1, 0, 0),
        (46, 0, 0),
        (i16::MAX, u8::MAX, i32::MIN),
    ] {
        assert!(
            matches!(
                InventoryAction::from_click(7, 17, slot, button, mode, 1 << 44),
                InventoryAction::Reject { window_id: 7, predicted_slots } if predicted_slots == 1 << 44
            ),
            "accepted invalid click: {slot}, {button}, {mode}"
        );
    }
}
