use super::*;

#[test]
fn gameplay_damage_and_teleports_follow_time_and_life_boundaries() {
    let now = Instant::now();
    let spawn = Transform::default();
    let mut life = PlayerLife::new(spawn);
    let hit = Damage { kind: DamageKind::Game, amount: 4.0, source: None };
    assert_eq!(life.damage(hit, now), DamageResult::Ignored);
    life.rules.protected = false;
    assert_eq!(life.damage(hit, now), DamageResult::Hurt { health: 16.0, died: false });
    assert_eq!(life.damage(hit, now + Duration::from_millis(100)), DamageResult::Ignored);
    assert_eq!(
        life.damage(Damage { amount: 6.0, ..hit }, now + Duration::from_millis(200)),
        DamageResult::Hurt { health: 14.0, died: false }
    );
    assert_eq!(
        life.damage(Damage { amount: 20.0, ..hit }, now + DAMAGE_WINDOW),
        DamageResult::Hurt { health: 0.0, died: true }
    );
    assert_eq!(life.damage(hit, now + Duration::from_secs(1)), DamageResult::Ignored);
    assert!(!life.heal(20.0));
    assert!(life.respawn());
    assert!(!life.respawn());
    assert_eq!(life.health(), 20.0);
    let first = life.teleport(spawn).unwrap();
    let second = life.teleport(Transform { x: 2.0, ..spawn }).unwrap();
    assert!(!life.confirm(first.id));
    assert!(life.confirm(second.id));
    assert!(!life.confirm(second.id));
}

#[test]
fn fall_threshold_and_void_damage_are_not_multiplied_by_simulation_rate() {
    let now = Instant::now();
    let mut life = PlayerLife::new(Transform::default());
    life.rules.protected = false;
    life.rules.fall_damage = true;
    life.rules.void_damage = true;
    assert_eq!(life.landing_damage(0.0, now), DamageResult::Ignored);
    assert_eq!(life.landing_damage(1.0, now), DamageResult::Hurt { health: 19.0, died: false });
    life.reset();
    for tick in 0..120 {
        life.void_damage(true, now + Duration::from_nanos(tick * 1_000_000_000 / 120));
    }
    assert_eq!(life.health(), 12.0);
    life.reset();
    life.damage(Damage { kind: DamageKind::Game, amount: 8.0, source: None }, now);
    assert_eq!(life.void_damage(true, now + Duration::from_millis(400)), DamageResult::Ignored);
    assert_eq!(
        life.void_damage(true, now + Duration::from_millis(500)),
        DamageResult::Hurt { health: 8.0, died: false }
    );
}

#[test]
fn premature_and_repeated_login_packets_are_rejected() {
    let mut progress = LoginProgress::default();
    assert!(progress.advance(LoginProgress::Acknowledge, LoginProgress::Complete).is_err());
    progress.advance(LoginProgress::Start, LoginProgress::Encryption).unwrap();
    assert!(progress.advance(LoginProgress::Start, LoginProgress::Encryption).is_err());
    progress.advance(LoginProgress::Encryption, LoginProgress::Acknowledge).unwrap();
    progress.advance(LoginProgress::Acknowledge, LoginProgress::Complete).unwrap();
}

#[test]
fn idle_keepalives_have_independent_deadlines() {
    let now = Instant::now();
    let mut keepalive = Keepalive::new(now);
    let id = keepalive.tick(now).unwrap().unwrap();
    assert!(keepalive.acknowledge(id + 1).is_err());
    assert!(keepalive.tick(now + KEEPALIVE_INTERVAL).unwrap().is_none());
    assert!(keepalive.tick(now + KEEPALIVE_TIMEOUT).is_err());
    keepalive.acknowledge(id).unwrap();
    assert!(keepalive.tick(now + KEEPALIVE_INTERVAL).unwrap().is_some());
}
