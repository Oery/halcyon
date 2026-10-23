use super::*;

#[test]
fn block_entities_and_cracks_reconcile_reliably_and_replay_after_reload() {
    let base = test_world();
    let mut arena = Arena::default();
    let sign = halcyon_world::blocks::named("minecraft:oak_sign").unwrap().default;
    arena.edit(&base, [8, 64, 8], sign).unwrap();
    let mut observer = WorldObserver::default();
    observer.reconcile(&arena, &HashSet::from([(0, 0)]));
    let terrain = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&terrain), [8]);
    observer.sent();
    let entity = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&entity), [8, 6]);
    assert_eq!(observer.next_frame(&arena).unwrap().unwrap(), entity);
    observer.sent();
    assert!(observer.next_frame(&arena).unwrap().is_none());
    observer.base_sent((0, 0));
    observer.patch_column(&arena, (0, 0)).unwrap();
    observer.sent();
    assert_eq!(observer.patch_block_entities(&arena, (0, 0)).unwrap().unwrap(), entity);
    observer.sent();
    arena.cracks.insert(9, crate::interaction::Crack { pos: [8, 64, 8], stage: 3 });
    arena.step(&base, &Default::default());
    observer.reconcile(&arena, &HashSet::from([(0, 0)]));
    assert_eq!(ids(&observer.next_frame(&arena).unwrap().unwrap()), [5]);
    observer.sent();
    arena.cracks.clear();
    arena.step(&base, &Default::default());
    observer.reconcile(&arena, &HashSet::from([(0, 0)]));
    assert_eq!(ids(&observer.next_frame(&arena).unwrap().unwrap()), [5]);
    observer.sent();
    arena.edit(&base, [8, 64, 8], 0).unwrap();
    assert_eq!(ids(&observer.next_frame(&arena).unwrap().unwrap()), [8]);
    observer.sent();
    assert_eq!(ids(&observer.next_frame(&arena).unwrap().unwrap()), [8]);
    observer.sent();
    assert!(observer.next_frame(&arena).unwrap().is_none());
}
use crate::arena::test_world;
use mc_protocol::{packets::PacketWriter, types::VarInt};

#[test]
fn crack_delivery_coalesces_ticks_and_clears_obsolete_pending_and_moved_targets() {
    use crate::interaction::Crack;
    let base = test_world();
    let mut arena = Arena::default();
    let mut observer = WorldObserver::default();
    let resident = HashSet::from([(0, 0)]);
    let old = [8, 64, 8];
    arena.cracks.insert(9, Crack { pos: old, stage: 1 });
    observer.reconcile(&arena, &resident);
    let pending = observer.next_frame(&arena).unwrap().unwrap();
    assert!(observer.cracks.is_empty());
    arena.cracks.clear();
    arena.step(&base, &Default::default());
    observer.reconcile(&arena, &resident);
    assert_eq!(observer.next_frame(&arena).unwrap().unwrap(), pending);
    observer.sent();
    assert_eq!(crack(&observer.next_frame(&arena).unwrap().unwrap()), (9, old, 255));
    observer.sent();
    assert!(observer.next_frame(&arena).unwrap().is_none());
    arena.cracks.insert(9, Crack { pos: old, stage: 2 });
    arena.step(&base, &Default::default());
    observer.reconcile(&arena, &resident);
    assert_eq!(crack(&observer.next_frame(&arena).unwrap().unwrap()), (9, old, 2));
    observer.sent();
    let new = [9, 64, 8];
    arena.cracks.insert(9, Crack { pos: new, stage: 4 });
    arena.step(&base, &Default::default());
    observer.reconcile(&arena, &resident);
    assert_eq!(crack(&observer.next_frame(&arena).unwrap().unwrap()), (9, old, 255));
    observer.sent();
    // A queued update uses current authority if its stage advances again.
    arena.cracks.insert(9, Crack { pos: new, stage: 5 });
    assert_eq!(crack(&observer.next_frame(&arena).unwrap().unwrap()), (9, new, 5));
    observer.sent();
    assert_eq!(observer.cracks.get(&9), arena.cracks.get(&9));
    observer.unloaded((0, 0));
    observer.reconcile(&arena, &HashSet::new());
    assert!(observer.next_frame(&arena).unwrap().is_none());
}

fn crack(bytes: &[u8]) -> (i32, BlockPos, u8) {
    let mut bytes = bytes;
    VarInt::read(&mut bytes).unwrap();
    assert_eq!(VarInt::read(&mut bytes).unwrap().0, 5);
    let id = VarInt::read(&mut bytes).unwrap().0;
    let pos = crate::arena::unpack(i64::read(&mut bytes).unwrap());
    let stage = u8::read(&mut bytes).unwrap();
    assert!(bytes.is_empty());
    (id, pos, stage)
}

fn ids(bytes: &[u8]) -> Vec<i32> {
    let mut input = bytes;
    let mut ids = Vec::new();
    while !input.is_empty() {
        let n = VarInt::read(&mut input).unwrap().0 as usize;
        let mut body = &input[..n];
        ids.push(VarInt::read(&mut body).unwrap().0);
        input = &input[n..];
    }
    ids
}
#[test]
fn patches_precede_spawns_and_delivered_baselines_survive_retry_and_chunk_resend() {
    let base = test_world();
    let mut arena = Arena::default();
    let entity_ids = crate::instance::EntityIds::default();
    arena.edit(&base, [8, 70, 8], 127).unwrap();
    arena.step(&base, &entity_ids);
    arena.step(&base, &entity_ids);
    let mut observer = WorldObserver::default();
    let resident = HashSet::from([(0, 0)]);
    observer.reconcile(&arena, &resident);
    let patches = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&patches), [8]);
    assert_eq!(observer.next_frame(&arena).unwrap().unwrap(), patches);
    assert!(observer.columns.is_empty());
    observer.sent();
    let spawn = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&spawn), [1]);
    observer.sent();
    assert_eq!(observer.rendered.len(), 1);
    arena.step(&base, &entity_ids);
    observer.reconcile(&arena, &resident);
    let motion = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&motion), [0x23, 0x67]);
    observer.sent();
    observer.base_sent((0, 0));
    assert!(observer.patch_column(&arena, (0, 0)).unwrap().is_some());
    observer.sent();
    for _ in 0..80 {
        arena.step(&base, &entity_ids);
    }
    observer.reconcile(&arena, &resident);
    let patch = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&patch), [8]);
    observer.sent();
    let remove = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&remove), [0x4e]);
    observer.sent();
    assert!(observer.rendered.is_empty());
    assert!(observer.next_frame(&arena).unwrap().is_none());
    observer.unloaded((0, 0));
    observer.reconcile(&arena, &HashSet::new());
    assert!(observer.next_frame(&arena).unwrap().is_none());
    observer.reconcile(&arena, &resident);
    assert!(observer.next_frame(&arena).unwrap().is_some());
}
