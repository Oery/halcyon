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
