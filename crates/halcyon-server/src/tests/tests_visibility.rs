use super::frame::{movement_frame, rendered};
use super::*;
use crate::lobby::Profile;
use mc_protocol::packets::PacketWriter;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::PlayerInfoUpdate;
use mc_protocol::packets::client::{
    EntityPositionSync, MoveEntityPosition, RemoveEntities, SetEquipment, SpawnPlayer,
};
use mc_protocol::types::VarInt;

pub(super) fn player(id: i32, x: f64, revision: u64) -> PlayerSnapshot {
    let username = format!("player{id}");
    let uuid = Uuid::from_u128(id as u128 + 1);
    let info =
        encode(&PlayerInfoUpdate { uuid, username: &username, properties: Vec::new() }).unwrap();
    PlayerSnapshot {
        entity_id: id,
        profile: Rc::new(Profile { uuid, username, info }),
        transform: Transform { x, y: 64.0, z: 1.0, ..Transform::default() },
        skin_parts: 0x7f,
        sneaking: false,
        pose: Default::default(),
        swimming: false,
        sprinting: false,
        using: None,
        health: 20.0,
        equipment: Rc::new(std::array::from_fn(|_| mc_protocol::types::ItemStack::default())),
        life_generation: 0,
        visible: true,
        revision,
    }
}

fn packets(mut bytes: &[u8]) -> Vec<(i32, Vec<u8>)> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let length = VarInt::read(&mut bytes).unwrap().0 as usize;
        let mut payload = &bytes[..length];
        let id = VarInt::read(&mut payload).unwrap().0;
        out.push((id, payload.to_vec()));
        bytes = &bytes[length..];
    }
    out
}

#[test]
fn swimming_and_crawling_publish_independent_flags_and_pose_changes() {
    use mc_protocol::packets::client::{PlayerPose, PlayerStateMetadata};
    let standing = player(1, 0.5, 0);
    let mut swimming = standing.clone();
    swimming.pose = PlayerPose::Swimming;
    swimming.swimming = true;
    let frame = movement_frame(swimming.clone(), &rendered(&standing)).unwrap().unwrap();
    let encoded = packets(&frame.bytes);
    assert_eq!(encoded.len(), 1);
    assert_eq!(encoded[0].0, PlayerStateMetadata::ID.0);
    assert_eq!(encoded[0].1[3] & 16, 16);
    assert_eq!(encoded[0].1[6], 3);
    let mut crawling = swimming.clone();
    crawling.swimming = false;
    let frame = movement_frame(crawling.clone(), &rendered(&swimming)).unwrap().unwrap();
    let encoded = packets(&frame.bytes);
    assert_eq!(encoded[0].1[3] & 16, 0);
    assert_eq!(encoded[0].1[6], 3);
    let frame = movement_frame(standing, &rendered(&crawling)).unwrap().unwrap();
    assert_eq!(packets(&frame.bytes)[0].1[6], 0);
}

fn drain(observer: &mut Observer) -> Vec<(i32, Vec<u8>)> {
    let mut out = Vec::new();
    while let Some(bytes) = observer.next_frame().unwrap() {
        out.extend(packets(&bytes));
        observer.frame_sent();
    }
    assert_eq!(observer.queued_bytes(), 0);
    assert_eq!(observer.queued_frames(), 0);
    out
}

#[test]
fn tab_membership_terrain_visibility_and_unload_order() {
    let local = player(0, 1.0, 1);
    let near = player(1, 2.0, 1);
    let far = player(2, 160.0, 1);
    let registry = Registry::from_snapshots(vec![local.clone(), near.clone(), far.clone()], 1);
    let terrain = HashSet::from([(0, 0)]);
    let mut observer = Observer::default();
    observer.reconcile(&registry, 0, &terrain).unwrap();
    let delivered = drain(&mut observer);
    let profile_index =
        delivered.iter().rposition(|(id, _)| *id == PlayerInfoUpdate::ID.0).unwrap();
    let spawn_index = delivered.iter().position(|(id, _)| *id == SpawnPlayer::ID.0).unwrap();
    assert!(profile_index < spawn_index);
    assert_eq!(observer.roster_count(), 3);
    assert_eq!(observer.visible_count(), 1);

    observer.before_unload((0, 0)).unwrap();
    assert!(observer.has_unload_barrier());
    let removal = observer.next_frame().unwrap().unwrap();
    assert_eq!(packets(&removal)[0].0, RemoveEntities::ID.0);
    observer.frame_sent();
    assert!(!observer.has_unload_barrier());
    assert_eq!(observer.visible_count(), 0);
    assert_eq!(observer.roster_count(), 3);

    let departed = Registry::from_snapshots(vec![local, far], 2);
    observer.reconcile(&departed, 0, &HashSet::new()).unwrap();
    let delivered = drain(&mut observer);
    assert_eq!(delivered.iter().filter(|(id, _)| *id == PlayerInfoRemove::ID.0).count(), 1);
    assert_eq!(observer.roster_count(), 2);
}

#[test]
fn equipment_uses_delivered_baselines_and_respawn_recreates_the_entity() {
    use mc_protocol::types::ItemStack;
    let local = player(0, 1.0, 1);
    let mut near = player(1, 2.0, 1);
    near.equipment = Rc::new(std::array::from_fn(|slot| {
        if slot == 0 {
            ItemStack::plain(1, 1).unwrap()
        } else {
            ItemStack::default()
        }
    }));
    let terrain = HashSet::from([(0, 0)]);
    let mut observer = Observer::default();
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 1), 0, &terrain)
        .unwrap();
    assert!(drain(&mut observer).iter().any(|(id, _)| *id == SetEquipment::ID.0));
    near.equipment = Rc::new(std::array::from_fn(|slot| {
        if slot == 0 {
            ItemStack::plain(2, 1).unwrap()
        } else {
            ItemStack::default()
        }
    }));
    near.revision = 2;
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 2), 0, &terrain)
        .unwrap();
    let selected = observer.next_frame().unwrap().unwrap();
    near.equipment = Rc::new(std::array::from_fn(|slot| {
        if slot == 0 {
            ItemStack::plain(3, 1).unwrap()
        } else {
            ItemStack::default()
        }
    }));
    near.revision = 3;
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 3), 0, &terrain)
        .unwrap();
    assert_eq!(observer.next_frame().unwrap().unwrap(), selected);
    observer.frame_sent();
    assert!(drain(&mut observer).iter().any(|(id, _)| *id == SetEquipment::ID.0));
    assert_eq!(observer.rendered[&1].equipment, near.equipment);
    near.life_generation = 1;
    near.revision = 4;
    observer
        .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 4), 0, &terrain)
        .unwrap();
    assert!(drain(&mut observer).iter().any(|(id, _)| *id == RemoveEntities::ID.0));
    observer.reconcile(&Registry::from_snapshots(vec![local, near], 5), 0, &terrain).unwrap();
    let packets = drain(&mut observer);
    assert!(packets.iter().any(|(id, _)| *id == SpawnPlayer::ID.0));
    assert!(!packets.iter().any(|(id, _)| *id == PlayerInfoRemove::ID.0));
    assert_eq!(observer.roster_count(), 2);
}

#[test]
fn movement_coalesces_against_only_successfully_delivered_baseline() {
    let local = player(0, 1.0, 1);
    let terrain = HashSet::from([(0, 0), (2, 0)]);
    let mut observer = Observer::default();
    observer
        .reconcile(
            &Registry::from_snapshots(vec![local.clone(), player(1, 1.0, 1)], 1),
            0,
            &terrain,
        )
        .unwrap();
    drain(&mut observer);
    for revision in [2, 3] {
        observer
            .reconcile(
                &Registry::from_snapshots(
                    vec![local.clone(), player(1, revision as f64, revision)],
                    revision,
                ),
                0,
                &terrain,
            )
            .unwrap();
    }
    assert_eq!(observer.coalesced_updates(), 1);
    let bytes = observer.next_frame().unwrap().unwrap();
    assert!(Rc::ptr_eq(&bytes, &observer.next_frame().unwrap().unwrap()));
    assert_eq!(observer.rendered[&1].transform.x, 1.0);
    let encoded = packets(&bytes);
    assert_eq!(encoded[0].0, MoveEntityPosition::ID.0);
    let movement = MoveEntityPosition::decode_payload(&mut encoded[0].1.as_slice()).unwrap();
    assert_eq!(movement.dx, 2 * 4096);
    observer.frame_sent();
    assert_eq!(observer.rendered[&1].transform.x, 3.0);

    observer
        .reconcile(&Registry::from_snapshots(vec![local, player(1, 40.0, 4)], 4), 0, &terrain)
        .unwrap();
    assert_eq!(packets(&observer.next_frame().unwrap().unwrap())[0].0, EntityPositionSync::ID.0);
    observer.frame_sent();
    assert_eq!(observer.rendered[&1].transform.x, 40.0);

    // A writer budget can select a spawn but defer its write to a later
    // pass. New publications must survive that old spawn's commit.
    let mut staged = Observer::default();
    staged
        .reconcile(
            &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 1.0, 1)], 1),
            0,
            &terrain,
        )
        .unwrap();
    loop {
        staged.next_frame().unwrap().unwrap();
        if matches!(staged.active.as_ref().unwrap().delivery, Delivery::Spawn(_)) {
            break;
        }
        staged.frame_sent();
    }
    staged
        .reconcile(
            &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 2.0, 2)], 2),
            0,
            &terrain,
        )
        .unwrap();
    staged.frame_sent();
    let pending = packets(&staged.next_frame().unwrap().unwrap());
    let movement = MoveEntityPosition::decode_payload(&mut pending[0].1.as_slice()).unwrap();
    assert_eq!(movement.dx, 4096);
    staged.frame_sent();
    assert_eq!(staged.rendered[&1].transform.x, 2.0);
}

#[test]
fn relative_positions_follow_java_rounding_at_positive_and_negative_half_units() {
    for (old, next, delta) in [(-0.5, 0.5, 1), (-0.51, -0.49, 1), (0.49, 0.51, 1), (0.51, 0.49, -1)]
    {
        let previous = player(1, old / 4096.0, 1);
        let current = player(1, next / 4096.0, 2);
        let frame = movement_frame(current, &rendered(&previous)).unwrap().unwrap();
        let encoded = packets(&frame.bytes);
        let movement = MoveEntityPosition::decode_payload(&mut encoded[0].1.as_slice()).unwrap();
        assert_eq!(movement.dx, delta, "old={old} next={next}");
    }
}

#[test]
fn unpublished_spawn_and_selected_movement_are_cancelled_before_unload() {
    let local = player(0, 1.0, 1);
    let terrain = HashSet::from([(0, 0)]);
    let registry = Registry::from_snapshots(vec![local.clone(), player(1, 2.0, 1)], 1);
    let mut observer = Observer::default();
    observer.reconcile(&registry, 0, &terrain).unwrap();
    observer.before_unload((0, 0)).unwrap();
    assert!(drain(&mut observer).iter().all(|(id, _)| *id != SpawnPlayer::ID.0));
    assert_eq!(observer.visible_count(), 0);

    observer
        .reconcile(
            &Registry::from_snapshots(vec![local.clone(), player(1, 2.0, 2)], 2),
            0,
            &terrain,
        )
        .unwrap();
    drain(&mut observer);
    observer
        .reconcile(&Registry::from_snapshots(vec![local, player(1, 3.0, 3)], 3), 0, &terrain)
        .unwrap();
    assert!(observer.next_frame().unwrap().is_some());
    observer.before_unload((0, 0)).unwrap();
    let delivered = drain(&mut observer);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, RemoveEntities::ID.0);

    let mut selected = Observer::default();
    selected
        .reconcile(
            &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 2.0, 1)], 1),
            0,
            &terrain,
        )
        .unwrap();
    loop {
        selected.next_frame().unwrap().unwrap();
        if matches!(selected.active.as_ref().unwrap().delivery, Delivery::Spawn(_)) {
            break;
        }
        selected.frame_sent();
    }
    selected.reconcile(&Registry::from_snapshots(vec![player(0, 1.0, 1)], 2), 0, &terrain).unwrap();
    assert!(drain(&mut selected).iter().all(|(id, _)| *id != SpawnPlayer::ID.0));
    assert_eq!(selected.visible_count(), 0);
    assert_eq!(selected.roster_count(), 1);
}

#[test]
fn admissions_are_paced_per_publication_and_backlog_is_bounded() {
    let records: Vec<_> = (0..300).map(|id| player(id, 1.0, 1)).collect();
    let registry = Registry::from_snapshots(records.clone(), 1);
    let mut observer = Observer::default();
    observer.reconcile(&registry, 0, &HashSet::new()).unwrap();
    let delivered = drain(&mut observer);
    assert_eq!(delivered.len(), ROSTER_BATCHES_PER_PUBLICATION);
    observer.reconcile(&registry, 0, &HashSet::new()).unwrap();
    assert!(drain(&mut observer).is_empty());
    assert_eq!(observer.roster_count(), ROSTER_BATCHES_PER_PUBLICATION * ROSTER_BATCH_ENTRIES);
    observer.reconcile(&Registry::from_snapshots(records, 2), 0, &HashSet::new()).unwrap();
    drain(&mut observer);
    assert_eq!(observer.roster_count(), 300);

    let mut observer = Observer::default();
    assert!(matches!(observer.reserve(MAX_BYTES + 1, 1), Err(VisibilityError::Overflow)));
    assert_eq!(observer.queued_bytes(), 0);
    observer.reserve(MAX_BYTES, MAX_FRAMES).unwrap();
    assert!(matches!(observer.reserve(0, 1), Err(VisibilityError::Overflow)));
    assert!(matches!(observer.reserve(1, 0), Err(VisibilityError::Overflow)));
    assert_eq!(observer.queued_bytes(), MAX_BYTES);
    assert_eq!(observer.queued_frames(), MAX_FRAMES);
}

#[test]
fn continuously_moving_players_share_a_limited_writer_budget() {
    let players: Vec<_> = (0..80).map(|id| player(id, 1.0, 1)).collect();
    let mut registry = Registry::from_snapshots(players.clone(), 1);
    let terrain = HashSet::from([(0, 0)]);
    let mut observer = Observer::default();
    for _ in 0..6 {
        registry.publish_transforms([]);
        observer.reconcile(&registry, 0, &terrain).unwrap();
        drain(&mut observer);
    }
    assert_eq!(observer.visible_count(), 79);
    for tick in 0..3 {
        registry.publish_transforms(players.iter().map(|player| {
            let mut transform = player.transform;
            transform.x += 0.25 + tick as f64 * 0.01;
            (player.entity_id, transform)
        }));
        observer.reconcile(&registry, 0, &terrain).unwrap();
        let mut frames = 0;
        while frames < 32 && observer.next_frame().unwrap().is_some() {
            frames += observer.next_frame_count();
            observer.frame_sent();
        }
        assert_eq!(observer.motion_waiting.len(), observer.latest.len());
        // Leftover motion must keep the writer awaking for another slice.
        assert!(observer.has_pending());
    }
    assert!(observer.rendered.values().all(|player| player.transform.x > 1.0));
}

#[test]
fn roster_batches_concatenate_single_player_entries() {
    let players = [player(1, 1.0, 1), player(2, 2.0, 1)];
    let bodies: Vec<_> =
        players.iter().map(|p| frame::roster_entry(&p.profile.info).unwrap()).collect();
    let batch = frame::roster_frame(&bodies).unwrap();
    let decoded = packets(&batch);
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].0, PlayerInfoUpdate::ID.0);
    let expected: Vec<u8> = [&[0x09, 2][..], bodies[0], bodies[1]].concat();
    assert_eq!(decoded[0].1, expected);
    assert!(frame::roster_entry(&batch).is_none());
}
