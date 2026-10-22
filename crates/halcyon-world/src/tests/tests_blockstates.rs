use super::*;

#[test]
fn canonical_key_sorts_props() {
    let props = vec![
        ("waterlogged".to_owned(), "true".to_owned()),
        ("facing".to_owned(), "north".to_owned()),
    ];
    assert_eq!(
        canonical_key("minecraft:oak_stairs", &props),
        "minecraft:oak_stairs[facing=north,waterlogged=true]"
    );
    assert_eq!(canonical_key("minecraft:stone", &[]), "minecraft:stone");
}

#[test]
fn table_lookup_hits_and_misses() {
    assert_eq!(block_state_id("minecraft:air"), Some(0));
    assert_eq!(block_state_id("minecraft:stone"), Some(1));
    // First and last entries by key-byte order.
    assert_eq!(
        block_state_id("minecraft:acacia_button[face=ceiling,facing=east,powered=false]"),
        Some(12537)
    );
    assert_eq!(block_state_id("minecraft:zombie_wall_head[facing=west,powered=true]"), Some(12798));
    assert_eq!(block_state_id("minecraft:not_a_block"), None);
    // Unsorted prop order must not match; loader canonicalizes first.
    assert_eq!(block_state_id("minecraft:oak_stairs[waterlogged=true,facing=north]"), None);
}
