use super::*;

#[test]
fn item_limits_identity_and_component_wire_lengths() {
    let sword = ITEMS.iter().position(|item| item.0 == "minecraft:diamond_sword").unwrap() as i32;
    assert_eq!(ItemStack::plain(sword, 2).unwrap_err(), ItemError::InvalidCount);
    let mut item = ItemStack::new(
        sword,
        1,
        ItemComponents {
            damage: Some(5),
            unbreakable: Some(false),
            name: Some("Sword".into()),
            lore: vec!["Arena".into()],
            dye: Some(0x123456),
            glint: Some(true),
        },
    )
    .unwrap();
    let plain = ItemStack::plain(sword, 1).unwrap();
    assert!(!item.same_item(&plain));
    let bytes = futures_lite::future::block_on(async {
        let mut bytes = Vec::new();
        item.write(&mut bytes).await.unwrap();
        bytes
    });
    assert_eq!(item.body_len(), bytes.len());
    assert_eq!(&bytes[bytes.len() - 7..], &[1, 47, 0, 0x12, 0x34, 0x56, 4]);
    assert_eq!(item.take(1).count(), 1);
    assert!(item.is_empty());
    assert_eq!(item.body_len(), 1);
}
