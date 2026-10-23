use super::*;

#[test]
fn creative_flight_is_requested_revoked_and_reset_with_authoritative_abilities() {
    use glommio::channels::oneshot;
    let world = ground_world(&[]);
    let mut player = PlayerState::new(Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() });
    player.life.rules.protected = false;
    let mut proposals = ProposalQueue::default();
    let mut actions = ActionQueue::default();
    let now = Instant::now();
    actions.push(0, PlayerAction::SetFlying(true));
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert_eq!(update.game_mode.unwrap().ability_flags(), 0);
    let (reply, _) = oneshot::oneshot();
    actions.push(0, PlayerAction::SetGameMode { mode: GameMode::Creative, reply });
    actions.push(0, PlayerAction::SetFlying(true));
    actions.push(
        0,
        PlayerAction::Damage(Damage { kind: DamageKind::Game, amount: 30.0, source: None }),
    );
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert_eq!(update.game_mode.unwrap().ability_flags(), 0x0f);
    assert!(player.game_mode.flying);
    assert_eq!(player.life.health(), 20.0);
    actions.push(0, PlayerAction::Reset);
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(update.respawn);
    assert!(!update.game_mode.unwrap().flying);
    assert_eq!(update.game_mode.unwrap().ability_flags(), 0x0d);
    let teleport = update.teleport.unwrap();
    player.life.confirm(teleport.id);
    actions.push(1, PlayerAction::SetFlying(true));
    let (reply, _) = oneshot::oneshot();
    actions.push(1, PlayerAction::SetGameMode { mode: GameMode::Survival, reply });
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert_eq!(update.game_mode.unwrap().ability_flags(), 0);
    assert!(!player.game_mode.flying);
}

#[test]
fn game_mode_changes_are_authoritative_coalesced_and_retained_on_respawn() {
    use futures_lite::future::block_on;
    use glommio::channels::oneshot;
    let world = ground_world(&[]);
    let spawn = Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() };
    let mut player = PlayerState::new(spawn);
    let mut actions = ActionQueue::default();
    let mut proposals = ProposalQueue::default();
    let mut merged = PlayerUpdate::default();
    for mode in [GameMode::Creative, GameMode::Spectator] {
        let (reply, response) = oneshot::oneshot();
        assert!(actions.push(0, PlayerAction::SetGameMode { mode, reply }));
        assert_ne!(player.game_mode.current, mode);
        let (update, _) =
            player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
        assert_eq!(block_on(response).unwrap(), Ok(()));
        merged.merge(update);
    }
    assert_eq!(
        merged.game_mode.unwrap(),
        GameModeState {
            current: GameMode::Spectator,
            previous: Some(GameMode::Creative),
            flying: true
        }
    );
    let (reply, response) = oneshot::oneshot();
    actions.push(0, PlayerAction::SetGameMode { mode: GameMode::Spectator, reply });
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
    assert_eq!(block_on(response).unwrap(), Ok(()));
    assert!(update.game_mode.is_none());
    assert_eq!(player.game_mode.previous, Some(GameMode::Creative));

    actions.push(0, PlayerAction::Reset);
    let (reply, response) = oneshot::oneshot();
    actions.push(0, PlayerAction::SetGameMode { mode: GameMode::Survival, reply });
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
    assert_eq!(block_on(response).unwrap(), Err(PlayerActionError::StaleLife));
    assert!(update.respawn);
    assert_eq!(update.game_mode, merged.game_mode);
    assert_eq!(player.game_mode.current, GameMode::Spectator);
    // Switching identity does not replace the protected lobby's gameplay rules.
    assert!(player.life.rules.protected);
}

#[test]
fn spectators_always_fly_ignore_damage_and_items_and_enter_walls() {
    use glommio::channels::oneshot;
    let world = ground_world(&[(8, 0, 8, 1), (8, 1, 8, 1)]);
    let mut player = PlayerState::new(Transform { x: 8.5, y: 66.0, z: 8.5, ..Default::default() });
    player.life.rules.protected = false;
    let mut proposals = ProposalQueue::default();
    let mut actions = ActionQueue::default();
    let now = Instant::now();
    let (reply, _) = oneshot::oneshot();
    actions.push(0, PlayerAction::SetGameMode { mode: GameMode::Spectator, reply });
    actions.push(0, PlayerAction::SetFlying(false));
    actions.push(
        0,
        PlayerAction::Damage(Damage { kind: DamageKind::Game, amount: 30.0, source: None }),
    );
    actions.push(0, PlayerAction::UseItem(Some(InteractionHand::Main)));
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert_eq!(update.game_mode.unwrap().ability_flags(), 0x07);
    assert!(player.game_mode.flying);
    assert_eq!(player.life.health(), 20.0);
    assert_eq!(player.using(), None);
    // Destinations and authority inside terrain neither obstruct nor suffocate.
    let wall = Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() };
    actions.push(0, PlayerAction::Teleport(wall));
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    player.life.confirm(update.teleport.unwrap().id);
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(update.damage.is_none());
    let (reply, _) = oneshot::oneshot();
    actions.push(0, PlayerAction::SetGameMode { mode: GameMode::Survival, reply });
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(!update.game_mode.unwrap().flying);
    assert_eq!(update.damage.unwrap().kind, DamageKind::Suffocation);
}

#[test]
fn a_pending_teleport_rejects_game_mode_actions_without_mutation() {
    use glommio::channels::oneshot;
    let mut player = PlayerState::new(Transform::default());
    player.life.teleport(Transform::default()).unwrap();
    let mut actions = ActionQueue::default();
    let (reply, response) = oneshot::oneshot();
    actions.push(0, PlayerAction::SetGameMode { mode: GameMode::Creative, reply });
    let (update, _) = player.advance(
        &empty_world(),
        &mut ProposalQueue::default(),
        &mut actions,
        Instant::now(),
        false,
    );
    assert_eq!(futures_lite::future::block_on(response).unwrap(), Err(PlayerActionError::NotReady));
    assert!(update.game_mode.is_none());
    assert_eq!(player.game_mode, GameModeState::default());
}

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
            block_entities: vec![],
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
fn corrections_preserve_compact_authority_inside_low_clearance() {
    use crate::movement::{MovementProposal, PlayerPose};
    // Top slab: crouching clearance. Full ceiling: crawling clearance.
    // An adjacent wall triggers the existing authoritative correction path.
    for (ceiling, pose) in [(15250, PlayerPose::Crouching), (1, PlayerPose::Swimming)] {
        let world = ground_world(&[(8, 1, 8, ceiling), (9, 0, 8, 1)]);
        let spawn = Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() };
        let mut player = PlayerState::new(spawn);
        let mut proposals = ProposalQueue::default();
        let mut actions = ActionQueue::default();
        proposals.push(MovementProposal { transform: spawn, sneaking: true, sprinting: false });
        player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
        assert_eq!(player.movement.pose(), pose);
        proposals.push(MovementProposal {
            transform: Transform { x: 9.0, ..spawn },
            sneaking: false,
            sprinting: false,
        });
        let (update, movement) =
            player.advance(&world, &mut proposals, &mut actions, Instant::now(), false);
        assert!(movement.unwrap().correction);
        assert_eq!(update.teleport.unwrap().destination, player.movement.transform());
        assert_eq!(player.movement.pose(), pose);
        assert!(!player.movement.swimming());
        assert!(!player.movement.sneaking());
        assert!(proposals.is_empty());
    }
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
    let input = MovementInput { forward: true, jumping: true, ..Default::default() };
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
    assert!(player.movement_input.is_none());
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
    assert!(player.movement_input.is_none());
    player.life.confirm(update.teleport.unwrap().id);
    player.life.rules.protected = false;
    actions.push(player.life.generation, PlayerAction::Drive(Some(input)));
    actions.push(
        player.life.generation,
        PlayerAction::Damage(Damage { kind: DamageKind::Game, amount: 100.0, source: None }),
    );
    player.advance(&world, &mut proposals, &mut actions, now, false);
    assert!(!player.life.alive());
    assert!(player.movement_input.is_none());
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
    let (update, _) = player.advance(&world, &mut proposals, &mut actions, now, true);
    assert!(update.respawn && player.life.alive());
    assert!(player.movement_input.is_none());
    assert_eq!(player.movement.motion.velocity, [0.0; 3]);
}

#[test]
fn item_use_ends_with_the_selection_or_the_life() {
    let world = ground_world(&[]);
    let mut player = PlayerState::new(Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() });
    player.life.rules.protected = false;
    let mut actions = ActionQueue::default();
    let mut step = |player: &mut PlayerState, action| {
        actions.push(player.life.generation, action);
        player.advance(&world, &mut ProposalQueue::default(), &mut actions, Instant::now(), false);
    };
    step(&mut player, PlayerAction::UseItem(Some(InteractionHand::Main)));
    assert_eq!(player.using(), Some(InteractionHand::Main));
    step(&mut player, PlayerAction::Inventory(InventoryAction::from_selection(3)));
    assert_eq!(player.using(), None);
    step(&mut player, PlayerAction::UseItem(Some(InteractionHand::Off)));
    step(&mut player, PlayerAction::Inventory(InventoryAction::from_selection(4)));
    assert_eq!(player.using(), Some(InteractionHand::Off));
    let lethal = Damage { kind: DamageKind::Game, amount: 40.0, source: None };
    step(&mut player, PlayerAction::Damage(lethal));
    assert_eq!(player.using(), None);
}
