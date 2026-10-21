use super::*;

fn profile(uuid: u128) -> Rc<Profile> {
    Rc::new(Profile { uuid: Uuid::from_u128(uuid), username: "Player".into(), info: Rc::from([]) })
}

fn reserve(registry: &mut Registry, uuid: u128) -> Session {
    let input = Rc::new(Input::new(Rc::default()));
    let id = registry.reserve(profile(uuid), Transform::default(), 0, input.clone()).unwrap();
    Session { entity_id: id, input }
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
    let sessions: Vec<_> =
        (1..=MAX_PLAYERS).map(|uuid| reserve(&mut registry, uuid as u128)).collect();
    assert!(registry.capacity_payload_bytes() >= MAX_PLAYERS * std::mem::size_of::<Player>());
    let input = Rc::new(Input::new(Rc::default()));
    assert_eq!(
        registry.reserve(profile(2_000), Transform::default(), 0, input),
        Err(AdmissionError::Full)
    );
    drop(sessions);
    registry.cleanup();
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
