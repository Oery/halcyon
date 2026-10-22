use super::*;

#[test]
fn menu_slots_hotbar_selections_and_outside_clicks_have_distinct_domains() {
    assert_eq!(std::mem::size_of::<InventorySlot>(), 1);
    assert_eq!(std::mem::size_of::<HotbarSlot>(), 1);
    assert_eq!(InventorySlot::all().len(), InventorySlot::COUNT);
    for (index, slot) in InventorySlot::all().enumerate() {
        assert_eq!(InventorySlot::try_from(index), Ok(slot));
        assert_eq!(InventorySlot::try_from(slot.protocol_id()), Ok(slot));
        assert_eq!(slot.index(), index);
        assert_eq!(slot.mask(), 1u64 << index);
        assert_eq!(slot.is_crafting(), index < 5);
        assert_eq!(slot.is_armor(), (5..9).contains(&index));
        assert_eq!(slot.is_storage(), (9..45).contains(&index));
        assert_eq!(ClickTarget::try_from(slot.protocol_id()), Ok(ClickTarget::Slot(slot)));
        assert_eq!(slot.hotbar_position().is_some(), (36..45).contains(&index));
    }
    for index in 0..HotbarSlot::COUNT {
        let hotbar = HotbarSlot::try_from(index).unwrap();
        assert_eq!(HotbarSlot::try_from(index as i16), Ok(hotbar));
        assert_eq!(hotbar.protocol_id(), index as u8);
        assert_eq!(hotbar.inventory_slot().index(), 36 + index);
        assert_eq!(hotbar.inventory_slot().hotbar_position(), Some(hotbar));
    }
    for raw in [i16::MIN, -1000, -999, -1, 46, i16::MAX] {
        assert_eq!(InventorySlot::try_from(raw), Err(InventoryError::InvalidSlot));
    }
    for raw in [i16::MIN, -999, -1, 9, 36, i16::MAX] {
        assert_eq!(HotbarSlot::try_from(raw), Err(InventoryError::InvalidSelection));
    }
    for raw in [InventorySlot::COUNT, usize::MAX] {
        assert_eq!(InventorySlot::try_from(raw), Err(InventoryError::InvalidSlot));
    }
    for raw in [HotbarSlot::COUNT, usize::MAX] {
        assert_eq!(HotbarSlot::try_from(raw), Err(InventoryError::InvalidSelection));
    }
    assert_eq!(ClickTarget::try_from(-999), Ok(ClickTarget::Outside));
    for raw in [i16::MIN, -1000, -1, 46, i16::MAX] {
        assert_eq!(ClickTarget::try_from(raw), Err(InventoryError::InvalidSlot));
    }
    assert_eq!(
        STORAGE.iter().map(|slot| slot.index()).collect::<Vec<_>>(),
        (36..45).chain(9..36).collect::<Vec<_>>()
    );
    assert_eq!(
        [
            InventorySlot::HEAD,
            InventorySlot::CHEST,
            InventorySlot::LEGS,
            InventorySlot::FEET,
            InventorySlot::OFFHAND
        ]
        .map(|slot| slot.index()),
        [5, 6, 7, 8, 45]
    );
}
