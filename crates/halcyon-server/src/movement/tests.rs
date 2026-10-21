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
            shapes,
            uncertain,
            contextual: false,
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
    assert!(state.advance_with(&map, &mut queue).crouching);
    propose(&mut queue, position(0.5, 1.0, 0.5));
    let result = state.advance_with(&map, &mut queue);
    assert!(result.crouching);
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
