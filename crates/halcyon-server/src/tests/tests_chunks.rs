use super::*;
use halcyon_world::{LoadedWorld, World, loader::LoadedChunk};

#[test]
fn respawn_requeues_the_center_without_losing_a_batch_acknowledgement() {
    let now = Instant::now();
    let mut chunks = ChunkTracker::new(now);
    chunks.sent.insert((0, 0), Residency::Terrain);
    chunks.terrain.insert((0, 0));
    chunks.waiting = Some(now);
    chunks.resend_center(&world(), (0, 0));
    assert_eq!(chunks.waiting, Some(now));
    assert!(chunks.terrain.contains(&(0, 0)));
    assert!(chunks.pending.contains(&((0, 0), Residency::Terrain)));
    assert!(chunks.center_dirty);
}

fn world() -> World {
    World::from_loaded(LoadedWorld {
        chunks: (-4..=4)
            .map(|x| LoadedChunk {
                block_entities: vec![],
                x,
                z: 0,
                data: vec![],
                heights: [0; 37],
                wire_bytes: 0,
                sections: Box::new(std::array::from_fn(|_| halcyon_world::Section::default())),
            })
            .collect(),
        spawn: (0.0, 64.0, 0.0, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    })
}

#[test]
fn padding_is_replaced_with_terrain_and_distant_chunks_unload() {
    let world = world();
    let mut tracker = ChunkTracker::new(Instant::now());
    tracker.set_view_distance(2);
    tracker.reconcile(&world, (0, 0));
    assert_eq!(tracker.wanted.get(&(3, 0)), Some(&Residency::Padding));
    tracker.sent.clone_from(&tracker.wanted);
    tracker.reconcile(&world, (1, 0));
    assert!(tracker.pending.contains(&((3, 0), Residency::Terrain)));
    assert!(tracker.gone.contains(&(-3, 0)));
    tracker.reconcile(&world, (100, 100));
    assert!(tracker.pending.is_empty());
    assert_eq!(tracker.gone.len(), tracker.sent.len());
    assert!(tracker.center_dirty);
}

#[test]
fn chunk_feedback_is_bounded_and_requires_an_outstanding_batch() {
    let now = Instant::now();
    let mut tracker = ChunkTracker::new(now);
    assert!(tracker.acknowledge(4.0, now).is_err());
    tracker.waiting = Some(now);
    assert!(tracker.acknowledge(f32::NAN, now).is_err());
    tracker.acknowledge(1000.0, now).unwrap();
    assert_eq!(tracker.batch_size, MAX_BATCH);
    tracker.waiting = Some(now);
    tracker.acknowledge(0.1, now).unwrap();
    assert_eq!(tracker.batch_size, 1);
    assert!(tracker.next_batch >= now + Duration::from_millis(499));
    tracker.set_view_distance(255);
    assert_eq!(tracker.radius, MAX_VIEW_DISTANCE);
}
