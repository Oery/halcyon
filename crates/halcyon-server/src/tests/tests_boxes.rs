use super::*;
use halcyon_world::BlockView;

#[test]
fn layout_sits_above_the_terrain_and_fits_the_overlay() {
    let world = test_world();
    let layout = Layout::new(&world).expect("room for the arena");
    assert_eq!(layout.floor, 63 + CLEARANCE);
    let mut arena = crate::arena::Arena::default();
    arena.initialize(&world).unwrap();
    for batch in layout.cells.chunks(halcyon_world::blocks::MAX_EDIT_CELLS) {
        arena.edit_all(&world, batch).unwrap();
    }
    // Every start is on a floor block inside its box.
    let view = arena.view(&world);
    for index in 0..BOXES {
        for slot in 0..3 {
            let spawn = layout.spawn(index, slot);
            let [x, z] = [spawn.x.floor() as i32, spawn.z.floor() as i32];
            assert_ne!(view.block_at(x, layout.floor, z), Some(0));
            assert_eq!(view.block_at(x, layout.floor + 1, z), Some(0));
        }
    }
}

#[test]
fn layout_needs_loaded_terrain() {
    let mut world = test_world();
    world.spawn = (100.5, 64.0, 0.5, 0.0, 0.0);
    assert!(Layout::new(&world).is_none());
}

#[test]
fn pairing_fills_boxes_in_pairs_and_adds_an_odd_fighter_to_the_last() {
    assert_eq!(pairing(2), [0, 0]);
    assert_eq!(pairing(3), [0, 0, 0]);
    assert_eq!(pairing(5), [0, 0, 1, 1, 1]);
    assert_eq!(pairing(8), [0, 0, 1, 1, 2, 2, 3, 3]);
}
