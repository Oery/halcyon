use super::owner::register;
use super::*;
use crate::inventory::InventoryAction;
use crate::player::PlayerAction;
use std::collections::HashSet;
use std::task::{Context, Waker};
use std::time::Instant;

fn profile(uuid: u128) -> Rc<Profile> {
    Rc::new(Profile { uuid: Uuid::from_u128(uuid), username: "Player".into(), info: Rc::from([]) })
}

fn reserve(registry: &mut Registry, uuid: u128) -> Session {
    let input = Rc::new(Input::new(Rc::default()));
    let id = registry.reserve(profile(uuid), Transform::default(), 0, input.clone()).unwrap();
    Session { entity_id: id, input }
}

fn command_player(registry: &mut Registry, uuid: u128, name: &str) -> Session {
    let input = Rc::new(Input::new(Rc::default()));
    let profile =
        Rc::new(Profile { uuid: Uuid::from_u128(uuid), username: name.into(), info: Rc::from([]) });
    let spawn = Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() };
    let entity_id = registry.reserve(profile, spawn, 0, input.clone()).unwrap();
    let session = Session { entity_id, input };
    session.activate();
    registry.publish();
    session.input.ready.set(true);
    session
}

#[test]
fn teleport_commands_resolve_current_players_and_executor_relative_coordinates() {
    use crate::player::PlayerActionError;
    use halcyon_commands::{CommandArgument, Position};
    let world = gameplay_benchmarks::terrain(false, false);
    let mut registry = Registry::default();
    let sender = command_player(&mut registry, 1, "Player");
    let target = command_player(&mut registry, 2, "Other");
    let destination = Transform { x: 12.5, y: 64.0, z: 8.5, yaw: 90.0, ..Default::default() };
    registry.players.get_mut(&target.entity_id()).unwrap().state.movement.reset(destination);
    let (name, position, response) = registry
        .request_teleport(sender.entity_id(), None, TeleportDestination::Player("oThEr"))
        .unwrap();
    assert_eq!(name, "Player");
    assert_eq!(position, destination);
    assert_eq!(sender.accepted_transform().x, 8.5);
    registry.simulate(&world, Instant::now(), &mut Vec::new());
    assert_eq!(futures_lite::future::block_on(response).unwrap(), Ok(()));
    let teleport = sender.take_update().teleport.unwrap();
    assert_eq!(teleport.destination, destination);
    assert!(!sender.ready());
    sender.confirm_teleport(teleport.id);
    registry.simulate(&world, Instant::now(), &mut Vec::new());
    assert!(sender.ready());
    registry
        .players
        .get_mut(&target.entity_id())
        .unwrap()
        .state
        .movement
        .reset(Transform { yaw: 45.0, ..destination });
    let (_, position, response) = registry
        .request_teleport(
            sender.entity_id(),
            Some("Other"),
            TeleportDestination::Position(Position::parse("~1 ~2 ~").unwrap()),
        )
        .unwrap();
    assert_eq!([position.x, position.y, position.z], [13.5, 66.0, 8.5]);
    assert_eq!(position.yaw, 45.0);
    registry.simulate(&world, Instant::now(), &mut Vec::new());
    assert_eq!(futures_lite::future::block_on(response).unwrap(), Ok(()));
    assert_eq!(target.take_update().teleport.unwrap().destination, position);
    assert_eq!(sender.accepted_transform().x, 12.5);
    assert!(matches!(
        registry.request_teleport(sender.entity_id(), None, TeleportDestination::Player("missing")),
        Err(PlayerActionError::NotFound)
    ));
}

#[test]
fn teleport_commands_report_invalid_destinations_stale_lives_and_queue_limits() {
    use crate::player::PlayerActionError;
    use halcyon_commands::{CommandArgument, Position};
    let world = gameplay_benchmarks::terrain(false, false);
    let mut registry = Registry::default();
    let sender = command_player(&mut registry, 1, "Player");
    for coordinates in ["8.5 63 8.5", "1000 64 1000"] {
        let (_, _, response) = registry
            .request_teleport(
                sender.entity_id(),
                None,
                TeleportDestination::Position(Position::parse(coordinates).unwrap()),
            )
            .unwrap();
        registry.simulate(&world, Instant::now(), &mut Vec::new());
        assert_eq!(
            futures_lite::future::block_on(response).unwrap(),
            Err(PlayerActionError::InvalidDestination)
        );
        assert!(sender.take_update().teleport.is_none());
        assert_eq!(sender.accepted_transform().x, 8.5);
    }
    assert!(matches!(
        registry.request_teleport(
            sender.entity_id(),
            None,
            TeleportDestination::Position(Position::parse("30000000 64 0").unwrap())
        ),
        Err(PlayerActionError::InvalidDestination)
    ));
    let (_, _, response) = registry
        .request_teleport(
            sender.entity_id(),
            None,
            TeleportDestination::Position(Position::parse("10 64 8").unwrap()),
        )
        .unwrap();
    registry.players.get_mut(&sender.entity_id()).unwrap().state.life.reset();
    registry.simulate(&world, Instant::now(), &mut Vec::new());
    assert_eq!(
        futures_lite::future::block_on(response).unwrap(),
        Err(PlayerActionError::StaleLife)
    );
    for _ in 0..32 {
        assert!(sender.submit(PlayerAction::Inventory(InventoryAction::Resync)));
    }
    assert!(matches!(
        registry.request_teleport(
            sender.entity_id(),
            None,
            TeleportDestination::Position(Position::parse("10 64 8").unwrap())
        ),
        Err(PlayerActionError::QueueFull)
    ));
}

#[test]
fn creative_breaks_share_bounded_block_work_and_resynchronize_stale_predictions() {
    use crate::interaction::BlockBreak;
    use halcyon_world::BlockView;
    use mc_protocol::types::GameMode;
    let world = gameplay_benchmarks::terrain(false, false);
    let mut registry = Registry::default();
    let player = command_player(&mut registry, 1, "Player");
    registry.players.get_mut(&player.entity_id()).unwrap().state.game_mode.current =
        GameMode::Creative;
    for i in 0..8 {
        assert!(player.break_block(BlockBreak { sequence: i + 1, pos: [5 + i, 63, 8] }));
    }
    assert!(!player.break_block(BlockBreak { sequence: 9, pos: [13, 63, 8] }));
    registry.simulate(&world, Instant::now(), &mut Vec::new());
    let update = player.take_update();
    assert_eq!(update.block_ack, Some(4));
    assert_eq!(update.used_blocks.len(), 4);
    for x in 5..9 {
        assert_eq!(registry.arena.view(&world).block_at(x, 63, 8), Some(0));
    }
    registry.players.get_mut(&player.entity_id()).unwrap().state.life.generation += 1;
    registry.simulate(&world, Instant::now(), &mut Vec::new());
    let update = player.take_update();
    assert_eq!(update.block_ack, Some(8));
    assert_eq!(update.used_blocks.len(), 4);
    for x in 9..13 {
        assert_eq!(registry.arena.view(&world).block_at(x, 63, 8), Some(1));
        assert!(update.used_blocks.contains(&[x, 63, 8]));
    }
}

#[test]
fn command_targets_resolve_names_and_publish_owner_applied_modes() {
    use crate::player::PlayerActionError;
    use mc_protocol::types::GameMode;
    let mut registry = Registry::default();
    let player = command_player(&mut registry, 1, "Player");
    let other = command_player(&mut registry, 2, "Other");
    let (name, response) =
        registry.request_game_mode(player.entity_id(), Some("oThEr"), GameMode::Adventure).unwrap();
    assert_eq!(name, "Other");
    assert_eq!(registry.players[&other.entity_id()].state.game_mode.current, GameMode::Survival);
    registry.simulate(&gameplay_benchmarks::terrain(false, false), Instant::now(), &mut Vec::new());
    assert_eq!(futures_lite::future::block_on(response).unwrap(), Ok(()));
    let previous_revision = registry.roster_revision();
    assert!(registry.publish());
    assert!(registry.roster_revision() > previous_revision);
    assert_eq!(registry.get(other.entity_id()).unwrap().game_mode, GameMode::Adventure);
    assert_eq!(registry.get(player.entity_id()).unwrap().game_mode, GameMode::Survival);
    assert_eq!(registry.command_player_names("pL"), ["Player"]);
    assert!(matches!(
        registry.request_game_mode(player.entity_id(), Some("missing"), GameMode::Creative),
        Err(PlayerActionError::NotFound)
    ));

    let (name, response) =
        registry.request_game_mode(player.entity_id(), None, GameMode::Creative).unwrap();
    assert_eq!(name, "Player");
    drop(player);
    registry.cleanup();
    assert!(futures_lite::future::block_on(response).is_err());
}

#[test]
fn command_targets_bound_work_and_reject_full_or_unready_sessions() {
    use crate::player::PlayerActionError;
    use mc_protocol::types::GameMode;
    let mut registry = Registry::default();
    let player = command_player(&mut registry, 1, "Player");
    player.input.ready.set(false);
    assert!(matches!(
        registry.request_game_mode(player.entity_id(), None, GameMode::Creative),
        Err(PlayerActionError::NotReady)
    ));
    player.input.ready.set(true);
    for _ in 0..32 {
        assert!(player.submit(PlayerAction::Inventory(InventoryAction::Resync)));
    }
    assert!(matches!(
        registry.request_game_mode(player.entity_id(), None, GameMode::Creative),
        Err(PlayerActionError::QueueFull)
    ));
    let duplicate = command_player(&mut registry, 2, "PLAYER");
    assert!(matches!(
        registry.request_game_mode(player.entity_id(), Some("player"), GameMode::Creative),
        Err(PlayerActionError::Ambiguous)
    ));
    drop(duplicate);
    registry.cleanup();
    let _players: Vec<_> =
        (3..73).map(|id| command_player(&mut registry, id, &format!("Player{id}"))).collect();
    let names = registry.command_player_names("player");
    assert_eq!(names.len(), 64);
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(registry.command_player_names("😀").is_empty());
}

#[test]
fn teleport_confirmation_survives_a_full_action_queue() {
    let world = gameplay_benchmarks::terrain(false, false);
    let mut registry = Registry::default();
    let input = Rc::new(Input::new(Rc::default()));
    let spawn = Transform { x: 8.5, y: 64.0, z: 8.5, ..Default::default() };
    let id = registry.reserve(profile(1), spawn, 0, input.clone()).unwrap();
    let session = Session { entity_id: id, input };
    session.teleport(spawn);
    let mut notices = Vec::new();
    registry.simulate(&world, Instant::now(), &mut notices);
    let teleport = session.take_update().teleport.unwrap();
    for _ in 0..32 {
        assert!(session.submit(PlayerAction::Inventory(InventoryAction::Resync)));
    }
    assert!(!session.submit(PlayerAction::Inventory(InventoryAction::Resync)));
    session.confirm_teleport(teleport.id);
    registry.simulate(&world, Instant::now(), &mut notices);
    assert!(session.ready());
    registry.publish();
    assert_eq!(registry.active_count(), 1);
    assert_eq!(session.accepted_transform(), registry.get(id).unwrap().transform);
}

#[test]
fn activation_coalesces_input_and_updates_spatial_membership() {
    let mut registry = Registry::default();
    let session = reserve(&mut registry, 1);
    assert!(registry.get(session.entity_id()).is_none());
    assert_eq!(registry.active_count(), 0);
    session.publish_transform(Transform { x: -0.5, ..Transform::default() });
    session.publish_transform(Transform { x: 17.0, ..Transform::default() });
    session.publish_skin_parts(0x7f);
    session.activate();
    assert!(registry.publish());
    let snapshot = registry.get(session.entity_id()).unwrap();
    assert_eq!(snapshot.transform.x, 17.0);
    assert_eq!(snapshot.skin_parts, 0x7f);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(registry.candidates(&HashSet::from([(1, 0)])).len(), 1);
    assert!(registry.candidates(&HashSet::from([(-1, 0)])).is_empty());
    assert!(!registry.publish());
    session.publish_transform(Transform { x: -0.5, ..Transform::default() });
    assert!(registry.publish());
    assert!(registry.candidates(&HashSet::from([(1, 0)])).is_empty());
    assert_eq!(registry.candidates(&HashSet::from([(-1, 0)])).len(), 1);
    let fixture = Registry::from_snapshots(registry.players().cloned().collect(), 2);
    assert_eq!(fixture.publication(), 2);
    assert_eq!(fixture.candidates(&HashSet::from([(-1, 0)])).len(), 1);
    let mut fixture = fixture;
    fixture
        .publish_transforms([(session.entity_id(), Transform { x: -0.6, ..Transform::default() })]);
    assert_eq!(fixture.publication(), 3);
    assert_eq!(fixture.revision(), 3);
    assert_eq!(fixture.spatial_revision(), 2);
    assert_eq!(fixture.roster_revision(), 2);
    fixture.publish_transforms([]);
    assert_eq!(fixture.publication(), 4);
    assert_eq!(fixture.revision(), 3);
}

#[test]
fn dropping_guards_cleans_up_without_enqueuing_and_ids_are_not_reused() {
    let mut registry = Registry::default();
    let first = reserve(&mut registry, 1);
    let first_id = first.entity_id();
    first.activate();
    registry.publish();
    assert!(matches!(
        registry.reserve(profile(1), Transform::default(), 0, Rc::new(Input::new(Rc::default()))),
        Err(AdmissionError::DuplicateUuid)
    ));
    drop(first);
    assert!(registry.cleanup());
    assert_eq!(registry.active_count(), 0);
    assert!(registry.columns.is_empty());
    let second = reserve(&mut registry, 1);
    assert!(second.entity_id() > first_id);
    drop(second);
    assert!(!registry.cleanup());
    assert!(registry.players.is_empty());
}

#[test]
fn admission_bounds_include_invisible_reservations_and_abandoned_replies() {
    let mut registry = Registry::default();
    let (reply, response) = oneshot::oneshot();
    drop(response);
    register(
        &mut registry,
        Registration {
            profile: profile(1),
            transform: Transform::default(),
            skin_parts: 0,
            input: Rc::new(Input::new(Rc::default())),
            reply,
        },
    );
    assert!(registry.players.is_empty());
    registry.last_entity_id = i32::MAX;
    assert_eq!(
        registry.reserve(profile(1), Transform::default(), 0, Rc::new(Input::new(Rc::default()))),
        Err(AdmissionError::IdExhausted)
    );
}

#[test]
fn wakeups_coalesce_and_survive_a_cancelled_pending_wait() {
    let signal = Signal::default();
    let mut cx = Context::from_waker(Waker::noop());
    assert!(signal.poll(&mut cx).is_pending());
    // Dropping the waiting future does not consume a later notification.
    signal.notify();
    signal.notify();
    assert!(signal.poll(&mut cx).is_ready());
    assert!(signal.poll(&mut cx).is_pending());
}

#[test]
fn a_full_registration_queue_cannot_block_disconnect_cleanup() {
    let mut registry = Registry::default();
    let session = reserve(&mut registry, 1);
    session.activate();
    registry.publish();
    let (sender, _receiver) = local_channel::new_bounded(REGISTRATION_CAPACITY);
    for id in 0..REGISTRATION_CAPACITY {
        let (reply, _response) = oneshot::oneshot();
        let request = Registration {
            profile: profile(id as u128 + 2),
            transform: Transform::default(),
            skin_parts: 0,
            input: Rc::new(Input::new(Rc::default())),
            reply,
        };
        assert!(sender.try_send(request).is_ok());
    }
    assert!(sender.is_full());
    drop(session);
    assert!(registry.cleanup());
    assert_eq!(registry.active_count(), 0);
    assert!(registry.players.is_empty());
    assert!(sender.is_full());
}
