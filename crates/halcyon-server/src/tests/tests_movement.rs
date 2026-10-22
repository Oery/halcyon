use super::*;
use halcyon_world::World;
use std::collections::HashMap;

const FULL: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0; 3] }];
const SLAB: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }];

#[derive(Default)]
pub(super) struct Map {
    pub(super) blocks: HashMap<(i32, i32, i32), (&'static [Aabb], bool)>,
    pub(super) surfaces: HashMap<(i32, i32, i32), MotionInfo>,
    missing: bool,
}

impl Map {
    pub(super) fn floor() -> Self {
        let mut map = Self::default();
        for x in -4..=4 {
            for z in -4..=4 {
                map.blocks.insert((x, 0, z), (FULL, false));
            }
        }
        map
    }
}

impl Terrain for Map {
    fn contains_column(&self, _: i32, _: i32) -> bool {
        !self.missing
    }

    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell> {
        self.blocks.get(&(x, y, z)).map(|&(shapes, uncertain)| TerrainCell {
            fluid: Default::default(),
            shapes,
            offset: [0.0; 3],
            uncertain,
            contextual: false,
            climb: Default::default(),
            motion: self.surfaces.get(&(x, y, z)).copied().unwrap_or_default(),
        })
    }
}

fn position(x: f64, y: f64, z: f64) -> Transform {
    Transform { x, y, z, on_ground: true, ..Transform::default() }
}

fn propose(queue: &mut ProposalQueue, transform: Transform) {
    assert!(queue.push(MovementProposal { transform, sneaking: false, sprinting: false }));
}

#[test]
fn imported_spawn_preserves_clear_positions_and_recovers_bounded_obstructions() {
    use halcyon_world::loader::{LoadedChunk, LoadedWorld};
    use halcyon_world::{SECTION_COUNT, Section};

    let mut sections =
        Box::new(std::array::from_fn::<_, SECTION_COUNT, _>(|_| Section::Uniform(0)));
    sections[7] = Section::Uniform(1);
    let mut blocks = Box::new([0; 4096]);
    blocks[8 * 16 + 8] = 1;
    sections[8] = Section::Dense(blocks);
    let map = World::from_loaded(LoadedWorld {
        chunks: vec![LoadedChunk {
            x: 0,
            z: 0,
            data: vec![],
            heights: [0; 37],
            wire_bytes: 0,
            sections,
        }],
        spawn: (8.5, 64.0, 8.5, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    });
    let clear = position(4.5, 64.0, 4.5);
    assert_eq!(resolve_spawn(&map, clear), Some(clear));
    let obstructed = position(8.5, 64.0, 8.5);
    let recovered = resolve_spawn(&map, obstructed).unwrap();
    assert!(MovementState::new(recovered).supports_destination(&map, recovered));
    assert!(
        (recovered.x - obstructed.x).powi(2)
            + (recovered.y - obstructed.y).powi(2)
            + (recovered.z - obstructed.z).powi(2)
            <= 1.0
    );
    assert!(resolve_spawn(&map, position(f64::NAN, 64.0, 8.5)).is_none());
    assert!(resolve_spawn(&map, position(100.5, 64.0, 100.5)).is_none());
}

#[test]
fn floors_walls_and_ceilings_clip_without_trusting_ground_flag() {
    let mut map = Map::floor();
    map.blocks.insert((1, 1, 0), (FULL, false));
    map.blocks.insert((0, 3, 0), (FULL, false));
    let mut state = MovementState::new(position(0.5, 1.0, 0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(1.0, 1.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert!((result.transform.x - 0.7).abs() < EPSILON);
    assert!(result.transform.on_ground);
    propose(&mut queue, position(0.7, 1.5, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert!((result.transform.y - 1.2).abs() < EPSILON);
    assert!(!result.transform.on_ground);
    propose(&mut queue, position(0.7, 0.5, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert_eq!(result.transform.y, 1.0);
}

#[test]
fn slab_steps_and_low_clearance_use_supported_shape_heights() {
    let mut map = Map::floor();
    map.blocks.insert((1, 1, 0), (SLAB, false));
    let mut state = MovementState::new(position(0.5, 1.0, 0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(1.1, 1.5, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(!result.correction);
    assert_eq!(result.transform, position(1.1, 1.5, 0.5));
    state.reset(position(0.5, 1.0, 0.5));
    state.transform.on_ground = true;
    propose(&mut queue, position(1.1, 1.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert_eq!(result.transform.y, 1.5);
    assert_eq!(result.transform.x, 1.1);

    map.blocks.insert((1, 3, 0), (FULL, false));
    state.reset(position(0.5, 1.0, 0.5));
    state.transform.on_ground = true;
    propose(&mut queue, position(1.1, 1.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert!((result.transform.x - 0.7).abs() < EPSILON);
}

#[test]
fn open_trapdoors_and_dry_small_dripleaf_leave_jump_clearance() {
    let mut map = Map::default();
    for x in 39..=44 {
        for z in -36..=-31 {
            map.blocks.insert((x, 118, z), (FULL, false));
        }
    }
    for (x, y, z, id) in
        [(41, 119, -34, 8907), (41, 120, -34, 8907), (41, 122, -34, 8899), (42, 122, -33, 33764)]
    {
        let motion = motion_info(id);
        map.blocks.insert(
            (x, y, z),
            (state_info(id).collision_boxes(), motion.surface == Surface::Unsupported),
        );
        map.surfaces.insert((x, y, z), motion);
    }
    let start = position(41.85, 119.0, -33.2);
    let apex = position(start.x, 120.25220334025373, start.z);
    let mut state = MovementState::new(start);
    let mut queue = ProposalQueue::default();
    propose(&mut queue, apex);
    let result = state.advance_with(&map, &mut queue);
    assert!(!result.correction);
    assert_eq!(result.transform.y, apex.y);

    // Closing the overhead trapdoor creates a real horizontal obstruction.
    map.blocks.insert((41, 122, -34), (state_info(8911).collision_boxes(), false));
    state.reset(start);
    propose(&mut queue, apex);
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert!((result.transform.y - (122.0 - STANDING_HEIGHT)).abs() < EPSILON);
}

#[test]
fn crouch_stays_below_ceiling_and_edge_backoff_keeps_support() {
    const LOW_ROOF: &[Aabb] = &[Aabb { min: [0.0, 0.5, 0.0], max: [1.0; 3] }];
    let mut map = Map::floor();
    map.blocks.insert((0, 2, 0), (LOW_ROOF, false));
    let mut state = MovementState::new(position(0.5, 1.0, 0.5));
    let mut queue = ProposalQueue::default();
    assert!(queue.push(MovementProposal {
        transform: position(0.5, 1.0, 0.5),
        sneaking: true,
        sprinting: false,
    }));
    assert_eq!(state.advance_with(&map, &mut queue).pose, PlayerPose::Crouching);
    propose(&mut queue, position(0.5, 1.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert_eq!(result.pose, PlayerPose::Crouching);
    assert!(!result.sneaking);

    let mut map = Map::default();
    map.blocks.insert((0, 0, 0), (FULL, false));
    assert!(queue.push(MovementProposal {
        transform: position(1.6, 1.0, 0.5),
        sneaking: true,
        sprinting: false,
    }));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert!(result.transform.x < 1.3);
    assert!(result.transform.on_ground);
}

#[test]
fn ordered_turns_survive_burst_and_queue_overflow_requests_sync() {
    let mut map = Map::floor();
    map.blocks.insert((1, 1, 1), (FULL, false));
    let mut state = MovementState::new(position(0.5, 1.0, 0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(2.0, 1.0, 0.5));
    propose(&mut queue, position(2.5, 1.0, 0.5));
    propose(&mut queue, position(2.5, 1.0, 2.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(!result.correction);
    assert_eq!(result.transform, position(2.5, 1.0, 2.5));
    for _ in 0..PROPOSAL_CAPACITY {
        propose(&mut queue, result.transform);
    }
    assert!(!queue.push(MovementProposal {
        transform: result.transform,
        sneaking: false,
        sprinting: false,
    }));
    assert!(state.advance_with(&map, &mut queue).correction);
    assert!(queue.is_empty());
}

#[test]
fn huge_proposals_unknown_columns_and_invalid_coordinates_do_bounded_work() {
    let mut state = MovementState::new(position(-0.5, 1.0, -0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(1_000_000.0, 1.0, 1_000_000.0));
    let result = state.advance_with(&Map::floor(), &mut queue);
    assert!(result.correction);
    assert_eq!(result.collision_cells, 0);
    propose(&mut queue, position(-0.4, 1.0, -0.5));
    assert!(state.advance_with(&Map { missing: true, ..Map::default() }, &mut queue).correction);
    propose(&mut queue, position(f64::NAN, 1.0, -0.5));
    assert!(state.advance_with(&Map::floor(), &mut queue).correction);
}

#[test]
fn accepted_descents_emit_damage_and_unsupported_contacts_do_not_change_authority() {
    let mut state = MovementState::new(position(0.5, 5.0, 0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(0.5, 3.0, 0.5));
    propose(&mut queue, position(0.5, 1.0, 0.5));
    let result = state.advance_with(&Map::floor(), &mut queue);
    assert!(!result.correction);
    assert_eq!(result.landed_fall_damage, Some(1.0));

    let mut map = Map::floor();
    map.blocks.insert((0, 3, 0), (&[], true));
    state.reset(position(0.5, 5.0, 0.5));
    propose(&mut queue, position(0.5, 3.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.uncertain_motion && result.correction);
    assert_eq!(state.transform().y, 5.0);
    map.blocks.remove(&(0, 3, 0));
    propose(&mut queue, position(0.5, 1.0, 0.5));
    assert_eq!(state.advance_with(&map, &mut queue).landed_fall_damage, Some(1.0));
    propose(&mut queue, position(0.5, 5.0, 0.5));
    assert!(!state.advance_with(&map, &mut queue).correction);
    propose(&mut queue, position(0.5, 1.0, 0.5));
    assert_eq!(state.advance_with(&map, &mut queue).landed_fall_damage, Some(1.0));
}

#[test]
fn unsupported_media_cannot_enable_low_tunnels() {
    let mut map = Map::floor();
    // Water leads into a dry one-block-high tunnel.
    map.blocks.insert((0, 1, 0), (&[], true));
    for x in 0..=2 {
        map.blocks.insert((x, 2, 0), (FULL, false));
    }
    let mut state = MovementState::new(position(-0.5, 1.0, 0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(0.5, 1.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.correction);
    assert!(result.uncertain_motion);
    assert_eq!(state.transform(), position(-0.5, 1.0, 0.5));
}

#[test]
fn bamboo_collides_at_its_vanilla_column_offset() {
    struct Grove(u16);
    impl BlockView for Grove {
        fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
            Some(match (x, y, z) {
                (_, 0, _) => 1,
                (1, 1 | 2, 0) => self.0,
                _ => 0,
            })
        }
        fn contains_column(&self, _: i32, _: i32) -> bool {
            true
        }
    }
    let bamboo =
        halcyon_world::blockstates::block_state_id("minecraft:bamboo[age=0,leaves=none,stage=0]")
            .unwrap();
    let grove = Grove(bamboo as u16);
    // Official 26.3 offset puts this column's minimum X at 1.5229166746139526.
    let mut state = MovementState::new(position(0.5, 1.0, 0.5));
    let mut queue = ProposalQueue::default();
    propose(&mut queue, position(1.2, 1.0, 0.5));
    let result = state.advance(&grove, &mut queue);
    assert!(!result.correction && !result.uncertain_motion, "{result:?}");
    propose(&mut queue, position(1.4, 1.0, 0.5));
    let result = state.advance(&grove, &mut queue);
    assert!(result.correction && !result.uncertain_motion);
    assert!((result.transform.x - (1.5229166746139526 - HALF_WIDTH)).abs() < 1e-9);
}

/// Floor at y=0, a north-facing ladder column at x=0, z=1 against a wall.
struct LadderWall {
    ladder: u16,
    top: Option<u16>,
}

impl BlockView for LadderWall {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        Some(match (x, y, z) {
            (_, 0, _) | (_, _, 2) => 1,
            (0, 1..=5, 1) => self.ladder,
            (0, 6, 1) => self.top.unwrap_or(self.ladder),
            _ => 0,
        })
    }
    fn contains_column(&self, _: i32, _: i32) -> bool {
        true
    }
}

fn ladder_wall(top: Option<&str>) -> LadderWall {
    let id = |key: &str| halcyon_world::blockstates::block_state_id(key).unwrap() as u16;
    LadderWall { ladder: id("minecraft:ladder[facing=north,waterlogged=false]"), top: top.map(id) }
}

#[test]
fn ladder_descent_is_accepted_and_resets_fall_distance() {
    let air = LadderWall { ladder: 0, top: None };
    for (wall, damaged) in [(ladder_wall(None), false), (air, true)] {
        let mut state = MovementState::new(position(0.5, 6.0, 1.5));
        let mut queue = ProposalQueue::default();
        let mut result = None;
        for step in 1..=50 {
            let y = (6.0 - 0.1 * f64::from(step)).max(1.0);
            propose(&mut queue, Transform { on_ground: y == 1.0, ..position(0.5, y, 1.5) });
            let advance = state.advance(&wall, &mut queue);
            assert!(!advance.correction && !advance.uncertain_motion, "{advance:?}");
            result = advance.landed_fall_damage.or(result);
        }
        assert_eq!(result.is_some_and(|d| d > 0.0), damaged);
    }
}

#[test]
fn ladders_climb_hold_and_continue_through_matching_trapdoors() {
    let input = MovementInput { forward: true, ..Default::default() };
    for top in [
        None,
        Some(
            "minecraft:oak_trapdoor[facing=north,half=bottom,open=true,powered=false,waterlogged=false]",
        ),
    ] {
        let wall = ladder_wall(top);
        let mut state = MovementState::new(position(0.5, 1.0, 1.5));
        for _ in 0..60 {
            assert!(!state.step(&wall, input).correction);
        }
        // Each climbing tick moves (0.2 - 0.08) * 0.98 upward.
        assert!(state.transform().y > 6.5, "{top:?}: {}", state.transform().y);
    }
    let wall = ladder_wall(None);
    let mut state = MovementState::new(position(0.5, 1.0, 1.5));
    for _ in 0..10 {
        state.step(&wall, input);
    }
    // Upward climbing momentum decays first; then sneaking cancels the descent.
    let sneak = MovementInput { sneaking: true, ..Default::default() };
    for _ in 0..3 {
        state.step(&wall, sneak);
    }
    let held = state.transform().y;
    for _ in 0..10 {
        let result = state.step(&wall, sneak);
        assert!(!result.correction);
    }
    assert_eq!(state.transform().y, held);
}

#[test]
fn vines_accept_client_climbs_and_descents_and_support_input_driven_climbing() {
    let id = |key: &str| halcyon_world::blockstates::block_state_id(key).unwrap() as u16;
    for key in [
        "minecraft:vine[east=false,north=true,south=false,up=false,west=false]",
        "minecraft:cave_vines[age=0,berries=true]",
        "minecraft:cave_vines_plant[berries=false]",
        "minecraft:weeping_vines[age=3]",
        "minecraft:weeping_vines_plant",
        "minecraft:twisting_vines[age=25]",
        "minecraft:twisting_vines_plant",
    ] {
        let wall = LadderWall { ladder: id(key), top: None };
        let mut state = MovementState::new(position(0.5, 2.0, 1.5));
        let mut queue = ProposalQueue::default();
        propose(&mut queue, position(0.5, 2.1176, 1.5));
        let result = state.advance(&wall, &mut queue);
        assert!(!result.correction && !result.uncertain_motion, "{key}: {result:?}");
        assert_eq!(result.transform.y, 2.1176);

        let input = MovementInput { forward: true, ..Default::default() };
        for _ in 0..35 {
            let result = state.step(&wall, input);
            assert!(!result.correction && !result.uncertain_motion, "{key}: {result:?}");
        }
        assert!(state.transform().y > 5.0, "{key}: {:?}", state.transform());
        let sneak = MovementInput { sneaking: true, ..Default::default() };
        for _ in 0..3 {
            state.step(&wall, sneak);
        }
        let held = state.transform().y;
        for _ in 0..5 {
            assert!(!state.step(&wall, sneak).correction, "{key}");
        }
        assert_eq!(state.transform().y, held, "{key}");

        let mut state = MovementState::new(position(0.5, 6.0, 1.5));
        for step in 1..=50 {
            let y = (6.0 - 0.1 * f64::from(step)).max(1.0);
            propose(&mut queue, position(0.5, y, 1.5));
            let result = state.advance(&wall, &mut queue);
            assert!(!result.correction && !result.uncertain_motion, "{key}: {result:?}");
            assert!(result.landed_fall_damage.is_none_or(|damage| damage == 0.0), "{key}");
        }
    }
}
