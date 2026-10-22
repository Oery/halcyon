use super::*;
use crate::lifecycle::DamageKind;

const FALL: Damage = Damage { kind: DamageKind::Fall, amount: 1.0, source: None };

fn collect(
    cursor: &mut EffectCursor,
    log: &EffectLog,
    self_id: i32,
    resident: &HashSet<Column>,
) -> Vec<Rc<[u8]>> {
    let mut out = Vec::new();
    cursor.collect(log, self_id, |id| id != 9, resident, &mut out);
    out
}

#[test]
fn a_new_cursor_starts_at_the_head_and_then_follows_it() {
    let mut log = EffectLog::default();
    log.hurt(2, FALL);
    let mut cursor = EffectCursor::default();
    let resident = HashSet::new();
    assert!(collect(&mut cursor, &log, 1, &resident).is_empty());
    log.hurt(2, FALL);
    assert!(cursor.pending(&log));
    assert_eq!(collect(&mut cursor, &log, 1, &resident).len(), 1);
    assert!(!cursor.pending(&log));
}

#[test]
fn delivery_skips_the_actor_hidden_entities_and_distant_columns() {
    let mut log = EffectLog::default();
    let mut cursor = EffectCursor::default();
    let resident = HashSet::from([(0, 0)]);
    collect(&mut cursor, &log, 1, &resident);
    log.swing(2, InteractionHand::Main, Some(2));
    log.hurt(1, FALL); // The victim already received its own event.
    log.swing(9, InteractionHand::Main, None); // Not rendered by this observer.
    log.sound(Sound::ATTACK_STRONG, [40.0, 64.0, 0.0], 1.0, 1.0, 0, None);
    log.sound(Sound::ATTACK_WEAK, [4.0, 64.0, 4.0], 1.0, 1.0, 0, Some(2));
    let frames = collect(&mut cursor, &log, 1, &resident);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0][1], 0x7B);
    assert_eq!(frames[1][1], 0x77);
    // The actor sees neither its swing nor its excluded sound, only the hurt.
    let mut actor = EffectCursor { next: Some(0) };
    let frames = collect(&mut actor, &log, 2, &resident);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0][1], 0x19);
}

#[test]
fn a_lagging_cursor_skips_overwritten_effects_and_drains_in_slices() {
    let mut log = EffectLog::default();
    let mut cursor = EffectCursor::default();
    let resident = HashSet::new();
    collect(&mut cursor, &log, 1, &resident);
    for _ in 0..CAPACITY + 10 {
        log.hurt(2, FALL);
    }
    let mut delivered = 0;
    while cursor.pending(&log) {
        let frames = collect(&mut cursor, &log, 1, &resident);
        assert!(frames.len() <= PER_DRAIN);
        delivered += frames.len();
    }
    assert_eq!(delivered, CAPACITY);
}
