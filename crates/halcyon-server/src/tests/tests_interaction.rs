use super::*;
use crate::arena::test_world;
use crate::instance::Transform;
use halcyon_world::blockstates::block_state_id;
use mc_protocol::types::ItemStack;

fn held(player: &mut PlayerState, name: &str, count: u8) {
    let item = mc_protocol::data::items::ITEMS.iter().position(|(item, ..)| *item == name).unwrap();
    player
        .inventory
        .set_slot(
            player.inventory.selected().inventory_slot(),
            ItemStack::plain(item as i32, count).unwrap(),
        )
        .unwrap();
    player.equipment = std::rc::Rc::new(player.inventory.equipment());
}

#[test]
fn placement_checks_inventory_permissions_occupancy_and_consumes_only_on_commit() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 10.5, 0.0);
    held(&mut player, "minecraft:stone", 3);
    let request = used([8, 63, 8]);
    assert!(
        place_block(&mut arena, &base, &mut player, request, &[], &mut Default::default())
            .is_none()
    );
    player.life.rules.block_edits.survival = true;
    let occupant = halcyon_world::Aabb { min: [8.2, 64.0, 8.2], max: [8.8, 65.8, 8.8] };
    assert!(
        place_block(&mut arena, &base, &mut player, request, &[occupant], &mut Default::default())
            .is_none()
    );
    assert_eq!(player.inventory.stack(player.inventory.selected().inventory_slot()).count(), 3);
    assert!(
        place_block(&mut arena, &base, &mut player, request, &[], &mut Default::default())
            .is_some()
    );
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(1));
    assert_eq!(player.inventory.stack(player.inventory.selected().inventory_slot()).count(), 2);
    player.game_mode.current = GameMode::Creative;
    assert!(
        place_block(&mut arena, &base, &mut player, used([9, 63, 8]), &[], &mut Default::default())
            .is_some()
    );
    assert_eq!(player.inventory.stack(player.inventory.selected().inventory_slot()).count(), 2);
}

#[test]
fn slabs_merge_and_doors_place_together_with_support_updates() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 10.5, 0.0);
    player.game_mode.current = GameMode::Creative;
    held(&mut player, "minecraft:stone_slab", 2);
    assert!(
        place_block(&mut arena, &base, &mut player, used([8, 63, 8]), &[], &mut Default::default())
            .is_some()
    );
    assert!(
        place_block(&mut arena, &base, &mut player, used([8, 64, 8]), &[], &mut Default::default())
            .is_some()
    );
    assert_eq!(
        halcyon_world::blocks::property(arena.view(&base).block_at(8, 64, 8).unwrap(), "type"),
        Some("double")
    );
    held(&mut player, "minecraft:oak_door", 1);
    assert!(
        place_block(&mut arena, &base, &mut player, used([9, 63, 8]), &[], &mut Default::default())
            .is_some()
    );
    assert_eq!(
        halcyon_world::blocks::property(arena.view(&base).block_at(9, 65, 8).unwrap(), "half"),
        Some("upper")
    );
    let cells = arena.edit_all(&base, &[([9, 63, 8], 0)]).unwrap();
    assert!(cells.contains(&[9, 64, 8]) && cells.contains(&[9, 65, 8]));
    assert_eq!(arena.view(&base).block_at(9, 65, 8), Some(0));
}

#[test]
fn fences_walls_and_stairs_update_against_the_final_neighbor_states() {
    use halcyon_world::blocks::{named, property, with};
    let base = test_world();
    let mut arena = Arena::default();
    let fence = named("minecraft:oak_fence").unwrap().default;
    arena.edit_all(&base, &[([8, 64, 8], fence), ([9, 64, 8], 1)]).unwrap();
    assert_eq!(property(arena.view(&base).block_at(8, 64, 8).unwrap(), "east"), Some("true"));
    arena.edit(&base, [9, 64, 8], 0).unwrap();
    assert_eq!(property(arena.view(&base).block_at(8, 64, 8).unwrap(), "east"), Some("false"));
    let wall = named("minecraft:cobblestone_wall").unwrap().default;
    arena.edit_all(&base, &[([10, 64, 8], wall), ([11, 64, 8], 1)]).unwrap();
    assert_eq!(property(arena.view(&base).block_at(10, 64, 8).unwrap(), "east"), Some("low"));
    arena.edit(&base, [10, 65, 8], 1).unwrap();
    assert_eq!(property(arena.view(&base).block_at(10, 64, 8).unwrap(), "east"), Some("tall"));
    let stair = named("minecraft:stone_stairs").unwrap().default;
    arena
        .edit_all(
            &base,
            &[
                ([6, 64, 8], with(stair, &[("facing", "north")]).unwrap()),
                ([6, 64, 7], with(stair, &[("facing", "west")]).unwrap()),
            ],
        )
        .unwrap();
    assert_eq!(
        property(arena.view(&base).block_at(6, 64, 8).unwrap(), "shape"),
        Some("outer_left")
    );
}

#[test]
fn wall_signs_place_against_a_solid_clicked_face() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 10.5, 180.0);
    player.game_mode.current = GameMode::Creative;
    held(&mut player, "minecraft:oak_sign", 1);
    arena.edit(&base, [8, 64, 8], 1).unwrap();
    let request = BlockUse { face: 3, ..used([8, 64, 8]) };
    assert!(
        place_block(&mut arena, &base, &mut player, request, &[], &mut Default::default())
            .is_some()
    );
    assert_eq!(
        halcyon_world::blocks::metadata(arena.view(&base).block_at(8, 64, 9).unwrap()).name,
        "minecraft:oak_wall_sign"
    );
}

#[test]
fn mining_waits_for_authority_and_cancels_after_tool_changes() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 10.5, 0.0);
    player.life.rules.block_edits.survival = true;
    held(&mut player, "minecraft:wooden_pickaxe", 1);
    let target = [8, 64, 8];
    arena.edit(&base, target, 1).unwrap();
    assert!(handle_break(&mut arena, &base, &mut player, 7, broken(target)).is_some());
    assert!(
        handle_break(&mut arena, &base, &mut player, 7, BlockBreak { action: 3, ..broken(target) })
            .is_some()
    );
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(1));
    let mut ticks = 1;
    while player.mining.is_some() && ticks < 40 {
        arena.step(&base, &mut 100);
        advance_mining(&mut arena, &base, &mut player, 7);
        ticks += 1;
    }
    // hardness 1.5, wooden pick speed 2: ceil(1.5 * 30 / 2) = 23 steps.
    assert_eq!(ticks, 23);
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(0));
    assert!(!arena.cracks.contains_key(&7));
    arena.edit(&base, target, 1).unwrap();
    handle_break(&mut arena, &base, &mut player, 7, broken(target));
    held(&mut player, "minecraft:stone_pickaxe", 1);
    advance_mining(&mut arena, &base, &mut player, 7);
    assert!(player.mining.is_none());
    assert!(!arena.cracks.contains_key(&7));
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(1));
}

#[test]
fn jukeboxes_consume_play_stop_and_discard_their_disc() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 10.5, 0.0);
    let pos = [8, 64, 8];
    arena
        .edit(&base, pos, halcyon_world::blocks::named("minecraft:jukebox").unwrap().default)
        .unwrap();
    held(&mut player, "minecraft:music_disc_cat", 1);
    assert!(
        use_jukebox(&mut arena, &base, &mut player, used(pos), &mut Default::default()).is_some()
    );
    assert!(player.inventory.stack(player.inventory.selected().inventory_slot()).is_empty());
    assert!(arena.block_entities.playing.contains_key(&pos));
    let until = arena.block_entities.playing[&pos].until;
    arena.block_entities.step(until);
    assert!(arena.block_entities.playing.is_empty());
    assert!(
        use_jukebox(&mut arena, &base, &mut player, used(pos), &mut Default::default()).is_some()
    );
    assert!(
        matches!(&arena.block_entities.record(pos).unwrap().data,Some(entities::EntityData::Jukebox(stack)) if stack.is_empty())
    );
    assert!(player.inventory.stack(player.inventory.selected().inventory_slot()).is_empty());
}

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
    BlockUse { face: 1, sequence: 1, pos, cursor: [0.5, 0.5, 0.5], main_hand: true }
}

fn broken(pos: BlockPos) -> BlockBreak {
    BlockBreak { action: 0, sequence: 1, pos }
}

#[test]
fn creative_breaking_is_instant_reach_checked_and_preserves_water_and_paired_blocks() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 10.5, 0.0);
    let target = [8, 64, 8];
    arena.edit(&base, target, 1).unwrap();
    assert!(break_block(&mut arena, &base, &player, broken(target)).is_none());
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(1));
    player.game_mode.current = GameMode::Creative;
    assert_eq!(break_block(&mut arena, &base, &player, broken(target)), Some(vec![target]));
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(0));

    arena.edit(&base, target, id("minecraft:stone_slab[type=bottom,waterlogged=true]")).unwrap();
    break_block(&mut arena, &base, &player, broken(target)).unwrap();
    let water = id("minecraft:water[level=0]");
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(water));
    assert!(break_block(&mut arena, &base, &player, broken(target)).is_none());

    arena
        .edit_all(&base, &[(target, door("lower", false)), ([8, 65, 8], door("upper", false))])
        .unwrap();
    assert_eq!(
        break_block(&mut arena, &base, &player, broken(target)),
        Some(vec![target, [8, 65, 8]])
    );
    assert_eq!(arena.view(&base).block_at(8, 65, 8), Some(0));
    for facing in ["north", "east", "south", "west"] {
        let offset = match facing {
            "north" => [0, 0, -1],
            "east" => [1, 0, 0],
            "south" => [0, 0, 1],
            _ => [-1, 0, 0],
        };
        let head = std::array::from_fn(|axis| target[axis] + offset[axis]);
        let foot_state =
            id(&format!("minecraft:red_bed[facing={facing},occupied=false,part=foot]"));
        let head_state =
            id(&format!("minecraft:red_bed[facing={facing},occupied=false,part=head]"));
        arena.edit_all(&base, &[(target, foot_state), (head, head_state)]).unwrap();
        assert_eq!(break_block(&mut arena, &base, &player, broken(head)), Some(vec![head, target]));
    }
    arena.edit(&base, [0, 64, 0], 1).unwrap();
    assert!(break_block(&mut arena, &base, &player, broken([0, 64, 0])).is_none());
    assert_eq!(arena.view(&base).block_at(0, 64, 0), Some(1));
    player.life.teleport(player.movement.transform()).unwrap();
    assert!(break_block(&mut arena, &base, &player, broken([8, 63, 8])).is_none());
}

#[test]
fn creative_breaking_respects_overlay_capacity_without_removing_the_target() {
    let base = test_world();
    let mut arena = Arena::default();
    let mut player = player(8.5, 8.5, 0.0);
    player.game_mode.current = GameMode::Creative;
    let mut exhausted = false;
    'fill: for x in 0..16 {
        for y in 65..69 {
            for z in 0..16 {
                match arena.edit(&base, [x, y, z], 1) {
                    Ok(()) => {}
                    Err(crate::arena::EditError::Capacity) => {
                        exhausted = true;
                        break 'fill;
                    }
                    other => panic!("unexpected edit result: {other:?}"),
                }
            }
        }
    }
    assert!(exhausted);
    let target = [8, 63, 8];
    let previous = arena.view(&base).block_at(8, 63, 8);
    assert!(break_block(&mut arena, &base, &player, broken(target)).is_none());
    assert_eq!(arena.view(&base).block_at(8, 63, 8), previous);
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
    arena
        .edit_all(&base, &[([8, 64, 8], door("lower", false)), ([8, 65, 8], door("upper", false))])
        .unwrap();
    let near = player(8.5, 10.5, 180.0);

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
