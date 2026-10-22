use super::*;
use crate::arena::test_world;
use mc_protocol::{packets::PacketWriter, types::VarInt};
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
    let mut id = 0;
    arena.edit(&base, [8, 70, 8], 127).unwrap();
    arena.step(&base, &mut id);
    arena.step(&base, &mut id);
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
    arena.step(&base, &mut id);
    observer.reconcile(&arena, &resident);
    let motion = observer.next_frame(&arena).unwrap().unwrap();
    assert_eq!(ids(&motion), [0x23, 0x67]);
    observer.sent();
    observer.base_sent((0, 0));
    assert!(observer.patch_column(&arena, (0, 0)).unwrap().is_some());
    observer.sent();
    for _ in 0..80 {
        arena.step(&base, &mut id);
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
