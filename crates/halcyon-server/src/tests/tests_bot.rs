use super::*;

fn standing() -> Transform {
    Transform { x: 0.5, y: 64.0, z: 0.5, ..Default::default() }
}

#[test]
fn rng_is_reproducible_and_bounded() {
    let mut a = Rng::new(7);
    let mut b = Rng::new(7);
    for _ in 0..1_000 {
        let value = a.range(-3.0, 3.0);
        assert_eq!(value, b.range(-3.0, 3.0));
        assert!((-3.0..3.0).contains(&value));
    }
}

#[test]
fn wander_turns_gradually() {
    let mut rng = Rng::new(1);
    let mut wander = Wander::new(0.0);
    let mut yaw = 0.0f32;
    for _ in 0..2_000 {
        let input = wander.tick(&mut rng, standing());
        // 18 degree rate cap plus tremor.
        let delta = (input.yaw - yaw + 540.0).rem_euclid(360.0) - 180.0;
        assert!(delta.abs() < 18.5);
        assert!((-180.0..=180.0).contains(&input.yaw));
        assert!((-90.0..=90.0).contains(&input.pitch));
        yaw = input.yaw;
    }
}

#[test]
fn blocked_walker_jumps_then_changes_heading() {
    let mut rng = Rng::new(3);
    let mut wander = Wander::new(0.0);
    while !wander.walking {
        wander.tick(&mut rng, standing());
    }
    let heading = wander.target_yaw;
    let mut jumped = false;
    for _ in 0..12 {
        jumped |= wander.tick(&mut rng, standing()).jumping;
    }
    assert!(jumped);
    assert_ne!(wander.target_yaw, heading);
}

#[test]
fn identical_inputs_are_not_resubmitted() {
    let input = MovementInput { forward: true, yaw: 10.0, ..Default::default() };
    assert!(same_input(input, input));
    assert!(!same_input(input, MovementInput { yaw: 10.5, ..input }));
}
