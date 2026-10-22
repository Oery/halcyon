use super::*;
use halcyon_world::{BlockView, blockstates::block_state_id};
use std::collections::HashMap;

#[derive(Default)]
struct Map(HashMap<[i32; 3], u16>);
impl BlockView for Map {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        Some(self.0.get(&[x, y, z]).copied().unwrap_or(if y == 0 { 1 } else { 0 }))
    }
    fn contains_column(&self, _: i32, _: i32) -> bool {
        true
    }
}
fn id(key: &str) -> u16 {
    block_state_id(key).unwrap() as u16
}
fn at(x: f64, y: f64, z: f64) -> Transform {
    Transform { x, y, z, ..Default::default() }
}
fn predict(state: &mut MovementState, map: &Map, target: Transform, sneak: bool) -> Advance {
    let mut queue = ProposalQueue::default();
    queue.push(MovementProposal { transform: target, sneaking: sneak, sprinting: false });
    state.advance(map, &mut queue)
}

#[test]
fn cobweb_travel_clears_momentum_and_does_not_double_slow_predictions() {
    let mut map = Map::default();
    map.0.insert([0, 1, 0], id("minecraft:cobweb"));
    let mut state = MovementState::new(at(0.5, 1.4, 0.5));
    let result = predict(&mut state, &map, at(0.5, 1.3, 0.6), false);
    assert!(!result.correction && !result.uncertain_motion);
    assert_eq!(result.transform.z, 0.6);
    state.motion.velocity = [0.4, -0.4, 0.4];
    let result = state.step(&map, MovementInput::default());
    assert!(!result.correction);
    assert!((result.transform.x - 0.6).abs() < 1e-9);
    assert!((result.transform.y - 1.28).abs() < 1e-9);
    assert_eq!(state.motion.velocity[0], 0.0);
    assert_eq!(state.fall_distance, 0.0);
}

#[test]
fn scaffolding_supports_tops_climbs_and_sneak_descents_in_water_or_air() {
    for wet in [false, true] {
        let scaffold =
            id(&format!("minecraft:scaffolding[bottom=false,distance=0,waterlogged={wet}]"));
        let mut map = Map::default();
        for y in 1..=8 {
            map.0.insert([0, y, 0], scaffold);
        }
        let mut state = MovementState::new(at(0.5, 2.0, 0.5));
        let result = predict(&mut state, &map, at(0.5, 1.95, 0.5), false);
        assert!(result.correction && !result.uncertain_motion);
        assert_eq!(result.transform.y, 2.0);
        let result = predict(&mut state, &map, at(0.5, 1.95, 0.5), true);
        assert!(!result.correction && !result.uncertain_motion, "{result:?}");
        let result = predict(&mut state, &map, at(0.5, 2.1, 0.5), false);
        assert!(!result.correction, "{result:?}");
        if !wet {
            state.reset(at(0.5, 1.2, 0.5));
            for _ in 0..20 {
                assert!(
                    !state
                        .step(&map, MovementInput { jumping: true, ..Default::default() })
                        .correction
                );
            }
            assert!(state.transform.y > 3.0, "{:?}", state.transform);
            let before = state.transform.y;
            for _ in 0..8 {
                assert!(
                    !state
                        .step(&map, MovementInput { sneaking: true, ..Default::default() })
                        .correction
                );
            }
            assert!(state.transform.y < before);
        }
    }
    let cell = TerrainCell {
        fluid: Default::default(),
        shapes: &[],
        offset: [0.0; 3],
        uncertain: false,
        contextual: false,
        motion: motion_info(24065),
        climb: halcyon_world::Climb::Scaffolding,
    };
    let shapes = cell.collision_shapes(ShapeContext { feet: 1.2, ..Default::default() }, 1);
    assert_eq!(shapes[0].max[1], 0.125);
}

#[test]
fn powder_snow_sinks_supports_leather_boots_and_cushions_long_falls() {
    let mut map = Map::default();
    map.0.insert([0, 1, 0], id("minecraft:powder_snow"));
    for boots in [false, true] {
        let mut state = MovementState::new(at(0.5, 2.0, 0.5));
        state.set_powder_snow_walk(boots);
        for _ in 0..5 {
            let result = state.step(&map, MovementInput::default());
            assert!(!result.correction && !result.uncertain_motion, "{result:?}");
        }
        if boots {
            assert_eq!(state.transform.y, 2.0);
        } else {
            assert!(state.transform.y < 1.9);
        }
        if boots {
            for _ in 0..3 {
                state.step(&map, MovementInput { sneaking: true, ..Default::default() });
            }
            assert!(state.transform.y < 2.0);
            let mut escaped = false;
            for _ in 0..12 {
                let result =
                    state.step(&map, MovementInput { jumping: true, ..Default::default() });
                assert!(!result.correction);
                escaped |= result.transform.y > 2.0;
            }
            assert!(escaped, "{:?}", state.transform);
        }
    }
    let mut state = MovementState::new(at(0.5, 6.0, 0.5));
    let result = predict(&mut state, &map, at(0.5, 2.2, 0.5), false);
    assert!(!result.correction);
    let result = predict(&mut state, &map, at(0.5, 1.9, 0.5), false);
    assert!(!result.correction, "{result:?}");
    assert_eq!(result.landed_fall_damage, Some(0.0));
    assert!((result.transform.y - (1.0 + f64::from(0.9f32))).abs() < 1e-7);
    state.step(&map, MovementInput::default());
    assert!(state.step(&map, MovementInput::default()).transform.y < 1.9);
    // Merely brushing snow with the body enables the leather-boot escape,
    // while a feet cell in air does not apply the snow's stuck multiplier.
    state.reset(at(1.1, 1.2, 0.5));
    state.set_powder_snow_walk(true);
    assert!(!predict(&mut state, &map, at(1.1, 1.2, 0.5), false).correction);
    assert!(state.motion.effects.powder_snow);
    assert_eq!(state.motion.effects.stuck, Stuck::None);
    assert!(!state.step(&map, MovementInput { jumping: true, ..Default::default() }).correction);
    assert!((state.motion.velocity[1] - 0.117_600_002_3).abs() < 1e-9);
}

#[test]
fn bubble_columns_apply_submerged_caps_and_surface_launches() {
    for (down, expected) in [(false, 0.12), (true, -0.06)] {
        let mut map = Map::default();
        let column = id(&format!("minecraft:bubble_column[drag={down}]"));
        for y in 1..=5 {
            map.0.insert([0, y, 0], column);
        }
        let mut state = MovementState::new(at(0.5, 2.0, 0.5));
        let result = state.step(&map, MovementInput::default());
        assert!(!result.correction && !result.uncertain_motion, "{result:?}");
        // A standing body touches two submerged bubble cells after water travel.
        assert!(
            (state.motion.velocity[1] - (expected - 0.005)).abs() < 1e-9,
            "{:?}",
            state.motion.velocity
        );
        for _ in 0..20 {
            assert!(!state.step(&map, MovementInput::default()).correction);
        }
    }
    let mut impulse = BubbleImpulse::default();
    for _ in 0..40 {
        impulse.push(false, false);
    }
    assert_eq!(impulse.apply(0.0), 0.7);
    impulse.push(true, true);
    assert!((impulse.apply(0.0) - 0.67).abs() < 1e-9);
    let mut map = Map::default();
    map.0.insert([0, 1, 0], id("minecraft:bubble_column[drag=false]"));
    for (y, expected) in [(1.5, 0.095), (1.9, 0.0215999984741211)] {
        let mut state = MovementState::new(at(0.5, y, 0.5));
        assert!(!state.step(&map, MovementInput::default()).correction);
        assert!((state.motion.velocity[1] - expected).abs() < 1e-9, "{:?}", state.motion.velocity);
    }
}

#[test]
fn closed_shulkers_dripleaf_tilts_spikes_and_piston_placeholders_use_client_shapes() {
    let mut map = Map::default();
    for (key, height) in [
        ("minecraft:red_shulker_box[facing=up]", 1.0),
        ("minecraft:big_dripleaf[facing=north,tilt=none,waterlogged=false]", 0.9375),
        ("minecraft:big_dripleaf[facing=north,tilt=partial,waterlogged=false]", 0.8125),
    ] {
        map.0.insert([0, 1, 0], id(key));
        let mut state = MovementState::new(at(0.5, 2.0, 0.5));
        let result = predict(&mut state, &map, at(0.5, 1.0, 0.5), false);
        assert!(result.correction && !result.uncertain_motion, "{key}: {result:?}");
        assert!((result.transform.y - (1.0 + height)).abs() < 1e-9, "{key}");
    }
    for key in [
        "minecraft:moving_piston[facing=north,type=normal]",
        "minecraft:big_dripleaf[facing=north,tilt=full,waterlogged=false]",
    ] {
        map.0.insert([0, 1, 0], id(key));
        let mut state = MovementState::new(at(0.5, 2.0, 0.5));
        let result = predict(&mut state, &map, at(0.5, 1.0, 0.5), false);
        assert!(!result.correction && !result.uncertain_motion, "{key}: {result:?}");
    }
    for name in ["pointed_dripstone", "sulfur_spike"] {
        let state_id =
            id(&format!("minecraft:{name}[thickness=tip,vertical_direction=up,waterlogged=false]"));
        map.0.remove(&[0, 1, 0]);
        map.0.insert([7, 1, -11], state_id);
        let offset = halcyon_world::collision_offset(state_id, 7, -11);
        let mut state =
            MovementState::new(at(7.5 + offset[0] - 0.125, 2.0, -10.5 + offset[2] - 0.125));
        let p = state.transform;
        let result = predict(&mut state, &map, Transform { y: 1.0, ..p }, false);
        assert!(result.correction && !result.uncertain_motion, "{name}: {result:?}");
        assert_eq!(result.transform.y, 1.6875);
    }
    assert_eq!(landing_damage(5.0, motion_info(33570), false), 9.0);
}
