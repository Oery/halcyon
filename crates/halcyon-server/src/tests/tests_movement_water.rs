use super::*;
use halcyon_world::blockstates::block_state_id;
use std::collections::HashMap;

struct Pool {
    depth: i32,
    end: i32,
    blocks: HashMap<[i32; 3], u16>,
    water: u16,
    missing: bool,
}

impl Pool {
    fn new(depth: i32) -> Self {
        Self {
            depth,
            end: i32::MAX,
            blocks: HashMap::new(),
            water: id("minecraft:water[level=0]"),
            missing: false,
        }
    }
}

fn id(key: &str) -> u16 {
    block_state_id(key).unwrap() as u16
}

impl BlockView for Pool {
    fn contains_column(&self, _: i32, _: i32) -> bool {
        !self.missing
    }
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        Some(self.blocks.get(&[x, y, z]).copied().unwrap_or_else(|| {
            if y == 0 {
                1
            } else if (1..=self.depth).contains(&y) && x < self.end {
                self.water
            } else {
                0
            }
        }))
    }
}

fn at(x: f64, y: f64) -> Transform {
    Transform { x, y, z: 0.5, ..Default::default() }
}

// Independent reference: executed official 26.3 LivingEntity.travelInWater,
// moveRelative, jumpInLiquid and goDownInWater in an unobstructed source pool.
// Eight movement ticks, default attributes, real keyboard input normalization.
#[allow(clippy::excessive_precision)]
#[rustfmt::skip]
const WATER_REFERENCE: &[(&str, [f64; 6])] = &[
    ("sink", [0.500000000000000, 3.904028477980232, 0.500000000000000, 0.000000000000000, -0.020805696740116, 0.000000000000000]),
    ("walk", [0.500000000000000, 3.904028477980232, 0.957766696216257, 0.000000000000000, -0.020805696740116, 0.065246665760758]),
    ("rise", [0.500000000000000, 4.838246207177909, 0.957766696216257, 0.000000000000000, 0.112350761404534, 0.065246665760758]),
    ("descend", [0.500000000000000, 2.969810748782555, 0.637330020001620, 0.000000000000000, -0.153962154884766, 0.019574001315576]),
    ("diagonal", [0.565037552162439, 3.904028477980232, 0.962558928322238, 0.009269969752967, -0.020805696740116, 0.065929714940725]),
    ("sprint", [0.500000000000000, 4.000000000000000, 1.063344127717690, 0.000000000000000, 0.000000000000000, 0.100465573344065]),
    ("stop", [0.500000000000000, 3.904028477980232, 0.797203494782884, 0.000000000000000, -0.020805696740116, 0.018959304359867]),
    // Additional independent captures execute Player.travel's pitch steering
    // before the same LivingEntity water methods in an unobstructed deep pool.
    ("pitch_up", [0.500000000000000, 4.902903572226177, 1.063344127717690, 0.000000000000000, 0.122862950389699, 0.100465573344065]),
    ("pitch_down", [0.500000000000000, 2.767910593159308, 1.063344127717690, 0.000000000000000, -0.164625852878303, 0.100465573344065]),
    ("pitch_shallow_up", [0.500000000000000, 4.221677517822810, 1.063344127717690, 0.000000000000000, 0.030164853382542, 0.100465573344065]),
    ("pitch_shallow_down", [0.500000000000000, 3.778322482177188, 1.063344127717690, 0.000000000000000, -0.030164853382542, 0.100465573344065]),
];

#[test]
fn water_trajectories_match_official_client_methods() {
    let pool = Pool::new(9);
    for &(mode, expected) in WATER_REFERENCE {
        let mut state = MovementState::new(at(0.5, 4.0));
        for tick in 0..8 {
            let result = state.step(
                &pool,
                MovementInput {
                    forward: mode != "sink" && !(mode == "stop" && tick >= 4),
                    left: mode == "diagonal",
                    yaw: if mode == "diagonal" { 37.0 } else { 0.0 },
                    jumping: mode == "rise",
                    sneaking: mode == "descend",
                    sprinting: mode == "sprint" || mode.starts_with("pitch"),
                    pitch: match mode {
                        "pitch_up" => -45.0,
                        "pitch_down" => 45.0,
                        "pitch_shallow_up" => -10.0,
                        "pitch_shallow_down" => 10.0,
                        _ => 0.0,
                    },
                    ..Default::default()
                },
            );
            assert!(!result.correction, "{mode} tick {tick}: {result:?}");
            assert!(result.collision_cells <= CELLS_PER_STEP);
        }
        let p = state.transform();
        let [vx, vy, vz] = state.motion.velocity;
        for (actual, expected) in [p.x, p.y, p.z, vx, vy, vz].into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-12, "{mode}: {actual} != {expected}");
        }
    }
}

#[test]
fn water_surfaces_eyes_waterlogged_shapes_and_current_neighbors_are_distinct() {
    let mut pool = Pool::new(1);
    let mut cells = 0;
    let above = water_contact(&pool, at(0.5, 1.9), PlayerPose::Standing, true, &mut cells).unwrap();
    assert!(!above.touching());
    let submerged =
        water_contact(&pool, at(0.5, 1.2), PlayerPose::Swimming, true, &mut cells).unwrap();
    assert!(submerged.touching() && submerged.eyes);
    assert_eq!(submerged.current, [0.0; 3]);
    pool.blocks.insert([0, 1, 0], id("minecraft:stone_slab[type=bottom,waterlogged=true]"));
    let mut state = MovementState::new(at(0.5, 3.0));
    let mut queue = ProposalQueue::default();
    queue.push(MovementProposal { transform: at(0.5, 1.0), sneaking: false, sprinting: false });
    let result = state.advance(&pool, &mut queue);
    assert!(result.correction);
    assert_eq!(result.transform.y, 1.5);
    assert!(!result.uncertain_motion);
    assert!(state.supports_destination(&pool, at(0.5, 1.5)));
}

#[test]
fn swim_to_dry_crawl_preserves_clearance_and_recovers_standing() {
    let mut pool = Pool::new(3);
    pool.end = 1;
    for x in 1..=3 {
        pool.blocks.insert([x, 2, 0], 1);
    }
    let mut state = MovementState::new(at(0.5, 1.0));
    let mut queue = ProposalQueue::default();
    queue.push(MovementProposal { transform: at(0.5, 1.0), sneaking: false, sprinting: true });
    let result = state.advance(&pool, &mut queue);
    assert_eq!(result.pose, PlayerPose::Swimming);
    assert!(result.swimming && !result.correction);
    for x in [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5] {
        queue.push(MovementProposal {
            transform: at(x, 1.0),
            sneaking: false,
            sprinting: x <= 1.0,
        });
        let result = state.advance(&pool, &mut queue);
        assert!(!result.correction, "{x}: {result:?}");
        if (1.0..=3.5).contains(&x) {
            assert_eq!(result.pose, PlayerPose::Swimming);
        }
        if x >= 1.5 {
            assert!(!result.swimming);
        }
    }
    assert_eq!(state.pose(), PlayerPose::Standing);
}

#[test]
fn swimming_steers_vertically_and_only_water_cancels_fall_damage() {
    let pool = Pool::new(9);
    for pitch in [-45.0, 45.0] {
        let mut state = MovementState::new(at(0.5, 4.0));
        let result = state.step(
            &pool,
            MovementInput { forward: true, sprinting: true, pitch, ..Default::default() },
        );
        assert!(!result.correction);
        assert_eq!(result.pose, PlayerPose::Swimming);
        assert_eq!(result.transform.y > 4.0, pitch < 0.0);
        assert!(result.swimming);
    }
    for (depth, damage) in [(0, 4.0), (2, 0.0)] {
        let mut state = MovementState::new(at(0.5, 8.0));
        let mut queue = ProposalQueue::default();
        queue.push(MovementProposal { transform: at(0.5, 1.0), sneaking: false, sprinting: false });
        let result = state.advance(&Pool::new(depth), &mut queue);
        assert!(!result.correction);
        assert_eq!(result.landed_fall_damage.unwrap_or_default(), damage);
    }
}

#[test]
fn fluid_queries_fail_boundedly_and_resets_clear_swimming() {
    let mut pool = Pool::new(9);
    let mut state = MovementState::new(at(0.5, 4.0));
    let input = MovementInput { forward: true, sprinting: true, ..Default::default() };
    assert!(state.step(&pool, input).swimming);
    let stopped =
        state.step(&pool, MovementInput { sprinting: true, pitch: 45.0, ..Default::default() });
    assert!(!stopped.swimming && !stopped.sprinting);
    state.reset(at(0.5, 4.0));
    assert!(!state.swimming());
    assert_eq!(state.pose(), PlayerPose::Standing);
    assert_eq!(state.motion.velocity, [0.0; 3]);
    let mut cells = CELLS_PER_STEP - 1;
    assert!(water_contact(&pool, at(0.5, 4.0), PlayerPose::Standing, true, &mut cells).is_none());
    assert_eq!(cells, CELLS_PER_STEP);
    pool.missing = true;
    assert!(state.step(&pool, input).correction);
    assert_eq!(state.transform(), at(0.5, 4.0));
}

#[test]
fn prone_water_travel_keeps_water_speed_while_dry_crawling_slows() {
    for (depth, sprinting) in [(1, false), (0, true)] {
        let mut pool = Pool::new(depth);
        for x in -2..=2 {
            for z in -2..=2 {
                pool.blocks.insert([x, 2, z], 1);
            }
        }
        let mut state = MovementState::new(at(0.5, 1.0));
        let result =
            state.step(&pool, MovementInput { forward: true, sprinting, ..Default::default() });
        assert!(!result.correction);
        assert_eq!(result.pose, PlayerPose::Swimming);
        assert!(!result.swimming && !result.sprinting);
        if depth == 1 {
            assert!((result.transform.z - 0.5196000007).abs() < 1e-9);
        } else {
            assert!(result.transform.z < 0.51);
        }
    }
}

#[test]
fn flowing_water_pushes_and_falling_water_pulls_beside_sturdy_walls() {
    let mut pool = Pool::new(9);
    for y in 1..=9 {
        pool.blocks.insert([1, y, 0], id("minecraft:water[level=7]"));
    }
    let mut cells = 0;
    let contact =
        water_contact(&pool, at(0.5, 4.0), PlayerPose::Standing, true, &mut cells).unwrap();
    assert_eq!(contact.current, [1.0, 0.0, 0.0]);
    let mut velocity = [0.0; 3];
    contact.push(&mut velocity);
    assert_eq!(velocity, [0.014, 0.0, 0.0]);
    pool.water = id("minecraft:water[level=8]");
    for y in 1..=9 {
        pool.blocks.insert([1, y, 0], 1);
    }
    cells = 0;
    let contact =
        water_contact(&pool, at(0.5, 4.0), PlayerPose::Standing, true, &mut cells).unwrap();
    assert_eq!(contact.current, [0.0, -1.0, 0.0]);
    assert!(cells < CELLS_PER_STEP);
}

#[test]
fn shoreline_clearance_allows_exit_without_a_step_or_flight_exception() {
    let mut pool = Pool::new(1);
    pool.end = 1;
    for x in 1..=8 {
        for z in -2..=2 {
            pool.blocks.insert([x, 1, z], 1);
        }
    }
    let mut state = MovementState::new(at(0.5, 1.5));
    state.motion.velocity[0] = 0.25;
    let result =
        state.step(&pool, MovementInput { forward: true, yaw: -90.0, ..Default::default() });
    assert!(!result.correction && result.collisions[0]);
    assert_eq!(state.motion.velocity[1], f64::from(0.3f32));
    for _ in 0..20 {
        let result =
            state.step(&pool, MovementInput { forward: true, yaw: -90.0, ..Default::default() });
        assert!(!result.correction, "{result:?}");
    }
    assert!(state.transform().x > 1.3);
    assert_eq!(state.transform().y, 2.0);
    assert_eq!(state.pose(), PlayerPose::Standing);
}
