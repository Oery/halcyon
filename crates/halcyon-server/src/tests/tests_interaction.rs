use super::*;
use crate::arena::test_world;
use crate::lobby::Transform;
use halcyon_world::blockstates::block_state_id;

fn id(key: &str) -> BlockStateId {
    block_state_id(key).unwrap() as BlockStateId
}

fn door(half: &str, open: bool) -> BlockStateId {
    id(&format!(
        "minecraft:oak_door[facing=north,half={half},hinge=left,open={open},powered=false]"
    ))
}

fn gate(facing: &str, open: bool) -> BlockStateId {
    id(&format!(
        "minecraft:oak_fence_gate[facing={facing},in_wall=false,open={open},powered=false]"
    ))
}

fn player(x: f64, z: f64, yaw: f32) -> PlayerState {
    PlayerState::new(Transform { x, y: 64.0, z, yaw, on_ground: true, ..Default::default() })
}

fn used(pos: BlockPos) -> BlockUse {
    BlockUse { sequence: 1, pos, cursor: [0.5, 0.5, 0.5], main_hand: true }
}

#[test]
fn doors_open_and_close_both_halves() {
    let base = test_world();
    let mut arena = Arena::default();
    arena
        .edit_all(&base, &[([8, 64, 8], door("lower", false)), ([8, 65, 8], door("upper", false))])
        .unwrap();
    let player = player(8.5, 10.5, 180.0);

    let cells = use_block(&mut arena, &base, &player, used([8, 65, 8])).unwrap();
    assert_eq!(cells, [[8, 65, 8], [8, 64, 8]]);
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(door("lower", true)));
    assert_eq!(arena.view(&base).block_at(8, 65, 8), Some(door("upper", true)));

    use_block(&mut arena, &base, &player, used([8, 64, 8])).unwrap();
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(door("lower", false)));
    assert_eq!(arena.view(&base).block_at(8, 65, 8), Some(door("upper", false)));
}

#[test]
fn unsupported_uses_change_nothing() {
    let base = test_world();
    let mut arena = Arena::default();
    arena.edit(&base, [8, 64, 8], door("lower", false)).unwrap();
    arena.edit(&base, [8, 65, 8], door("upper", true)).unwrap();
    let near = player(8.5, 10.5, 180.0);
    // Halves that disagree would be replaced by air in vanilla.
    assert_eq!(use_block(&mut arena, &base, &near, used([8, 64, 8])), None);
    arena.edit(&base, [8, 65, 8], door("upper", false)).unwrap();

    let far = player(8.5, 15.0, 180.0);
    assert_eq!(use_block(&mut arena, &base, &far, used([8, 64, 8])), None);
    let off_hand = BlockUse { main_hand: false, ..used([8, 64, 8]) };
    assert_eq!(use_block(&mut arena, &base, &near, off_hand), None);
    let outside = BlockUse { cursor: [0.5, 1.6, 0.5], ..used([8, 64, 8]) };
    assert_eq!(use_block(&mut arena, &base, &near, outside), None);
    assert_eq!(use_block(&mut arena, &base, &near, used([8, 63, 8])), None);
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(door("lower", false)));
}

#[test]
fn gates_swing_away_from_a_user_in_front() {
    let base = test_world();
    let mut arena = Arena::default();
    arena.edit(&base, [8, 64, 8], gate("north", false)).unwrap();
    // Standing north of the gate and looking south (yaw 0).
    let front = player(8.5, 6.5, 0.0);
    use_block(&mut arena, &base, &front, used([8, 64, 8])).unwrap();
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(gate("south", true)));
    use_block(&mut arena, &base, &front, used([8, 64, 8])).unwrap();
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(gate("south", false)));
    // Looking the way the gate faces keeps its facing.
    use_block(&mut arena, &base, &front, used([8, 64, 8])).unwrap();
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(gate("south", true)));
    use_block(&mut arena, &base, &front, used([8, 64, 8])).unwrap();
    // Looking north at the south-facing gate swings it back.
    let back = player(8.5, 10.5, 180.0);
    use_block(&mut arena, &base, &back, used([8, 64, 8])).unwrap();
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(gate("north", true)));
}
