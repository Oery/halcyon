use super::*;
use crate::bot::test_map::{MapView, stone};
use crate::movement::MovementState;

#[test]
fn rays_hit_boxes_in_front_only() {
    let target = player_bounds(Transform { x: 0.0, y: 0.0, z: 2.0, ..Default::default() });
    let eye = [0.0, EYE, 0.0];
    let ahead = ray_distance(eye, look_direction(0.0, 0.0), target).unwrap();
    assert!((ahead - 1.7).abs() < 1e-9);
    assert!(ray_distance(eye, look_direction(180.0, 0.0), target).is_none());
    assert!(ray_distance(eye, look_direction(90.0, 0.0), target).is_none());
    let (yaw, pitch) = look_at(eye, [0.0, 1.2, 2.0]);
    assert!(ray_distance(eye, look_direction(yaw, pitch), target).is_some());
}

#[test]
fn walls_block_aim_and_targeting() {
    let mut view = MapView::default();
    view.floor([-8, 8], 63, [-8, 8]);
    let me = Transform { x: 0.5, y: 64.0, z: 0.5, on_ground: true, ..Default::default() };
    let them = Transform { z: 3.0, ..me };
    let (yaw, pitch) = look_at([0.5, 64.0 + EYE, 0.5], [0.5, 65.2, 3.0]);
    assert!(aimed_at([0.5, 64.0 + EYE, 0.5], yaw, pitch, them, &view));
    for x in -8..=8 {
        for y in 64..67 {
            view.set([x, y, 2], stone());
        }
    }
    assert!(!aimed_at([0.5, 64.0 + EYE, 0.5], yaw, pitch, them, &view));
    assert!(!clear_path(&view, me, them));
}

/// Two fighters on a floor close in and land aimed swings, under the real
/// physics and motor.
#[test]
fn fighters_close_in_and_swing() {
    let mut view = MapView::default();
    view.floor([-16, 16], 63, [-16, 16]);
    let spawns = [
        Transform { x: 0.5, y: 64.0, z: -4.5, yaw: 0.0, on_ground: true, ..Default::default() },
        Transform { x: 0.5, y: 64.0, z: 4.5, yaw: 180.0, on_ground: true, ..Default::default() },
    ];
    let mut bodies = spawns.map(|spawn| {
        let mut state = MovementState::new(spawn);
        state.place(spawn);
        state
    });
    let mut fighters = [0, 1].map(|i| {
        let mut fighter = Fighter::new(BotProfile::new(70), 11 + i, spawns[i as usize]);
        fighter.engaged = true;
        fighter
    });
    let mut swings = [0; 2];
    for tick in 0..200 {
        let positions = bodies.each_ref().map(|body| body.transform());
        for i in 0..2 {
            let other =
                [Opponent { id: 1 - i as i32, transform: positions[1 - i], cues: Cues::default() }];
            let senses = Senses {
                tick,
                me: positions[i],
                health: 20.0,
                hurt: false,
                delay: 12.5,
                opponents: &other,
            };
            let decision = fighters[i].tick(&senses, &view);
            if decision.attack.is_some() {
                let eye = [positions[i].x, positions[i].y + EYE, positions[i].z];
                assert!(crate::combat::in_reach(eye, player_bounds(positions[1 - i]), false));
                swings[i] += 1;
            }
            bodies[i].step(&view, decision.input);
        }
    }
    // A full sword charge takes 12.5 ticks; early clicks are occasional.
    assert!(swings.iter().all(|&n| (3..=30).contains(&n)), "swings {swings:?}");
}
