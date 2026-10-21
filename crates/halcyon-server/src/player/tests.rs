use super::*;

fn empty_world() -> World {
    World::from_loaded(halcyon_world::LoadedWorld {
        chunks: Vec::new(),
        spawn: (0.0, 0.0, 0.0, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    })
}

fn ground_world(obstacles: &[(usize, usize, usize, u16)]) -> World {
    use halcyon_world::{LoadedWorld, SECTION_COUNT, Section, loader::LoadedChunk};
    let mut sections = Box::new(std::array::from_fn::<_, SECTION_COUNT, _>(|_| Section::default()));
    sections[7] = Section::Uniform(1);
    if !obstacles.is_empty() {
        let mut states = Box::new([0; 4096]);
        for &(x, y, z, id) in obstacles {
            states[(y << 8) | (z << 4) | x] = id;
        }
        sections[8] = Section::Dense(states);
    }
    World::from_loaded(LoadedWorld {
        chunks: vec![LoadedChunk {
            x: 0,
            z: 0,
            data: vec![],
            heights: [0; 37],
            wire_bytes: 0,
            sections,
        }],
        spawn: (8.5, 64.0, 8.5, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    })
}

#[test]
fn corrections_preserve_crouch_authority_inside_low_clearance() {
    use crate::movement::MovementProposal;
    // A top slab leaves 1.5 blocks of clearance; the adjacent block is a wall.
    let world = ground_world(&[(8, 1, 8, 15250), (9, 0, 8, 1)]);
    let spawn = Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() };
    let mut player = PlayerState::new(spawn);
    let mut proposals = ProposalQueue::default();
    let mut actions = ActionQueue::default();
    proposals.push(MovementProposal { transform: spawn, sneaking: true, sprinting: false });
    player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
    assert!(player.movement.crouching());
    proposals.push(MovementProposal {
        transform: Transform { x: 9.0, ..spawn },
        sneaking: false,
        sprinting: false,
    });
    let (update, movement) =
        player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
    assert!(movement.unwrap().correction);
    assert_eq!(update.teleport.unwrap().destination, player.movement.transform());
    assert!(player.movement.crouching());
    assert!(!player.movement.sneaking());
    assert!(proposals.is_empty());
}

#[test]
fn a_new_life_supersedes_undelivered_death_and_damage() {
    let damage = Damage { kind: DamageKind::Fall, amount: 30.0, source: None };
    let mut player = PlayerState::new(Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() });
    player.life.rules.protected = false;
    let mut actions = ActionQueue::default();
    actions.push(0, PlayerAction::Damage(damage));
    actions.push(0, PlayerAction::Reset);
    actions.push(0, PlayerAction::Damage(damage)); // Old-life work after reset is stale.
    let (newer, _) = player.advance(
        &ground_world(&[]),
        &mut ProposalQueue::default(),
        &mut actions,
        Instant::now(),
        false,
    );
    assert!(player.life.alive());
    assert_eq!(player.life.health(), 20.0);
    assert_eq!(player.life.generation, 1);
    let mut update = PlayerUpdate {
        health: Some(0.0),
        damage: Some(damage),
        death: Some(damage),
        ..Default::default()
    };
    update.merge(newer);
    assert!(update.death.is_none());
    assert!(update.damage.is_none());
    assert_eq!(update.health, Some(20.0));
    assert!(update.respawn);
}

#[test]
fn invalid_destinations_preserve_position_teleport_and_rules() {
    let spawn = Transform::default();
    let mut player = PlayerState::new(spawn);
    let pending = player.life.teleport(spawn).unwrap();
    for destination in [
        Transform { x: 30_000_000.1, ..spawn },
        Transform { y: -20_000_000.1, ..spawn },
        Transform { z: -30_000_000.1, ..spawn },
        Transform { x: f64::NAN, ..spawn },
        Transform { pitch: f32::INFINITY, ..spawn },
    ] {
        assert!(player.life.teleport(destination).is_none());
        let mut actions = ActionQueue::default();
        actions.push(0, PlayerAction::Teleport(destination));
        actions.push(
            0,
            PlayerAction::Rules(PlayerRules {
                protected: false,
                respawn: destination,
                ..player.life.rules
            }),
        );
        let (update, _) = player.advance(
            &empty_world(),
            &mut ProposalQueue::default(),
            &mut actions,
            Instant::now(),
            false,
        );
        assert!(!update.changed());
        assert_eq!(player.movement.transform(), spawn);
        assert_eq!(player.life.pending_teleport(), Some(pending));
        assert_eq!(player.life.rules.respawn, spawn);
        assert!(player.life.rules.protected);
    }
    for sign in [-1.0, 1.0] {
        let boundary = Transform {
            x: sign * 30_000_000.0,
            y: sign * 20_000_000.0,
            z: sign * 30_000_000.0,
            ..spawn
        };
        assert!(player.life.teleport(boundary).is_some());
    }
}

#[test]
fn driven_motion_and_queued_inputs_end_at_teleport_and_life_boundaries() {
    let world = ground_world(&[]);
    let spawn = Transform { x: 8.5, y: 64.0, z: 8.5, on_ground: true, ..Default::default() };
    let mut player = PlayerState::new(spawn);
    let input = GroundInput { forward: true, jumping: true, ..Default::default() };
    let mut actions = ActionQueue::default();
    let mut proposals = ProposalQueue::default();
    let now = Instant::now();
    actions.push(0, PlayerAction::Drive(Some(input)));
    player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(player.movement.motion.velocity[1] > 0.0);
    let destination = Transform { x: 10.5, ..spawn };
    actions.push(0, PlayerAction::Teleport(destination));
    for _ in 0..8 {
        actions.push(0, PlayerAction::Drive(Some(input)));
    }
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
    assert!(player.ground_input.is_none());
    assert!(actions.pending.is_empty());
    player.life.confirm(update.teleport.unwrap().id);
    player.advance(&world, &mut proposals, &mut actions, now, false);
    assert_eq!(player.movement.transform().x, destination.x);
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
    actions.push(0, PlayerAction::Drive(Some(input)));
    actions.push(0, PlayerAction::Reset);
    actions.push(0, PlayerAction::Drive(Some(input)));
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(update.respawn);
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
    assert!(player.ground_input.is_none());
    player.life.confirm(update.teleport.unwrap().id);
    player.life.rules.protected = false;
    actions.push(player.life.generation, PlayerAction::Drive(Some(input)));
    actions.push(
        player.life.generation,
        PlayerAction::Damage(Damage { kind: DamageKind::Game, amount: 100.0, source: None }),
    );
    player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(!player.life.alive());
    assert!(player.ground_input.is_none());
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, true);
    assert!(update.respawn && player.life.alive());
    assert!(player.ground_input.is_none());
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
}
