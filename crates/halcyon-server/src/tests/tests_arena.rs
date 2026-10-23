use super::*;

#[test]
fn rejected_multi_cell_edits_leave_revisions_schedules_and_terrain_untouched() {
    let base = test_world();
    let mut arena = Arena::default();
    assert!(arena.edit_all(&base, &[([8, 70, 8], 127), ([16, 64, 8], 1)]).is_err());
    assert_eq!(arena.revision, 0);
    assert_eq!(arena.changed, 0);
    assert!(arena.scheduled.is_empty() && arena.scheduled_at.is_empty());
    assert_eq!(arena.view(&base).block_at(8, 70, 8), Some(0));
}
use crate::{
    instance::Transform,
    movement::{MovementProposal, MovementState, ProposalQueue},
    player::{ActionQueue, PlayerState},
};
use std::time::{Duration, Instant};

#[test]
fn delayed_stack_falls_settles_and_preserves_authoritative_states() {
    let base = test_world();
    let mut arena = Arena::default();
    let ids = crate::instance::EntityIds::starting_after(100);
    arena.edit(&base, [8, 70, 8], 121).unwrap();
    arena.edit(&base, [8, 71, 8], 127).unwrap();
    arena.step(&base, &ids);
    assert!(arena.falling.is_empty());
    arena.step(&base, &ids);
    assert_eq!(arena.falling.len(), 2);
    assert_eq!(arena.falling[0].position[1], 69.96);
    assert!((arena.falling[0].velocity - (-0.04 * 0.98f32 as f64)).abs() < 1e-12);
    assert_eq!(arena.view(&base).block_at(8, 70, 8), Some(0));
    for _ in 0..80 {
        arena.step(&base, &ids);
    }
    assert!(arena.falling.is_empty());
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(121));
    assert_eq!(arena.view(&base).block_at(8, 65, 8), Some(127));
    assert_eq!(base.block_at(8, 64, 8), Some(0));
    assert_eq!(ids.last(), 102);
    arena.edit(&base, [8, 64, 8], 0).unwrap();
    arena.step(&base, &ids);
    arena.step(&base, &ids);
    assert!(!arena.falling.is_empty());
    for _ in 0..30 {
        arena.step(&base, &ids);
    }
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(127));
    for pos in [[-30_000_000, -64, 30_000_000], [8, 319, -8], [0, -1, 0]] {
        assert_eq!(unpack(pack(pos)), pos);
    }
}

#[test]
fn falling_trajectory_matches_official_gravity_and_move_methods() {
    // Captured independently from the target client's Entity.applyGravity,
    // Entity.move and getAirDrag, using an actual FallingBlockEntity.
    let base = test_world();
    let mut arena = Arena::default();
    let ids = crate::instance::EntityIds::starting_after(0);
    arena.edit(&base, [8, 79, 8], 127).unwrap();
    arena.step(&base, &ids);
    for tick in 1..=30 {
        arena.step(&base, &ids);
        let expected = match tick {
            1 => Some((15.96, -0.039200000762939)),
            5 => Some((15.415761898793507, -0.188315249119328)),
            10 => Some((13.926864808794297, -0.358537335717802)),
            20 => Some((8.574417918565551, -0.651488500003050)),
            _ => None,
        };
        if let Some((y, velocity)) = expected {
            assert!((arena.falling[0].position[1] - 63.0 - y).abs() < 1e-12);
            assert!((arena.falling[0].velocity - velocity).abs() < 1e-12);
        }
    }
    assert!(arena.falling.is_empty());
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(127));
}

#[test]
fn settled_blocks_suffocate_without_trapping_escape_or_bypassing_protection() {
    let base = test_world();
    let mut arena = Arena::default();
    arena.edit(&base, [8, 64, 8], 121).unwrap();
    arena.edit(&base, [8, 65, 8], 127).unwrap();
    let world = arena.view(&base);
    let start = Transform { x: 8.5, y: 64.0, z: 8.5, on_ground: true, ..Default::default() };
    let mut player = PlayerState::new(start);
    let mut queue = ProposalQueue::default();
    let mut actions = ActionQueue::default();
    let now = Instant::now();
    player.advance(&world, &mut queue, &mut actions, now, false);
    assert_eq!(player.life.health(), 20.0);
    player.life.rules.protected = false;
    player.advance(&world, &mut queue, &mut actions, now, false);
    assert_eq!(player.life.health(), 19.0);
    player.advance(&world, &mut queue, &mut actions, now + Duration::from_millis(499), false);
    assert_eq!(player.life.health(), 19.0);
    player.advance(&world, &mut queue, &mut actions, now + Duration::from_millis(500), false);
    assert_eq!(player.life.health(), 18.0);
    assert!(player.movement.suffocating(&world));
    queue.push(MovementProposal {
        transform: Transform { x: 9.5, ..start },
        sneaking: false,
        sprinting: false,
    });
    let (update, motion) =
        player.advance(&world, &mut queue, &mut actions, now + Duration::from_millis(550), false);
    assert!(!motion.unwrap().correction && update.teleport.is_none());
    assert!(!player.movement.suffocating(&world));
    assert_eq!(player.movement.transform().x, 9.5);
    // Transparent full cubes collide but do not suffocate.
    let glass = halcyon_world::blockstates::block_state_id("minecraft:glass").unwrap() as u16;
    arena.edit(&base, [8, 65, 8], glass).unwrap();
    assert!(!MovementState::new(start).suffocating(&arena.view(&base)));
    let obsidian = halcyon_world::blockstates::block_state_id("minecraft:obsidian").unwrap() as u16;
    arena.edit(&base, [8, 65, 8], obsidian).unwrap();
    assert!(MovementState::new(start).suffocating(&arena.view(&base)));
    let outside = Transform { x: 9.5, ..start };
    let mut movement = MovementState::new(outside);
    queue.push(MovementProposal { transform: start, sneaking: false, sprinting: false });
    let result = movement.advance(&arena.view(&base), &mut queue);
    assert!(result.correction && result.transform.x > 9.29);
}

#[test]
fn partial_landings_break_with_drops_disabled_and_capacity_failures_are_atomic() {
    let base = test_world();
    let mut arena = Arena::default();
    let ids = crate::instance::EntityIds::starting_after(0);
    arena.edit(&base, [8, 64, 8], 15252).unwrap(); // bottom slab
    arena.edit(&base, [8, 70, 8], 127).unwrap();
    for _ in 0..80 {
        arena.step(&base, &ids);
    }
    assert!(arena.falling.is_empty());
    assert_eq!(arena.view(&base).block_at(8, 65, 8), Some(0));
    assert_eq!(arena.view(&base).block_at(8, 64, 8), Some(15252));
    assert_eq!(arena.edit(&base, [8, 65, 8], u16::MAX), Err(EditError::Unsupported));
    arena.changed = MAX_CHANGED;
    assert_eq!(arena.edit(&base, [9, 65, 8], 127), Err(EditError::Capacity));
    assert_eq!(arena.view(&base).block_at(9, 65, 8), Some(0));
    let mut unsupported = Arena::default();
    unsupported.write([8, 64, 8], 25173); // honey's entity motion is unavailable
    assert_eq!(unsupported.edit(&base, [8, 70, 8], 127), Err(EditError::Unsupported));
    assert_eq!(unsupported.view(&base).block_at(8, 70, 8), Some(0));
}
