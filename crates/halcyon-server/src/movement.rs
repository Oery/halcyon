//! Bounded validation of client-predicted movement against immutable terrain.
//!
//! Coordinates remain proposals: this checks geometry and synchronizes stance,
//! rather than replaying client physics or detecting speed/flight cheats.
use std::collections::VecDeque;

use halcyon_world::{Aabb, MAX_BLOCK_Y, MIN_BLOCK_Y, World, state_info};

use crate::lobby::Transform;

const PROPOSAL_CAPACITY: usize = 32;
const PROPOSALS_PER_STEP: usize = 4;
const CELLS_PER_PROPOSAL: usize = 256;
const CELLS_PER_STEP: usize = 512;
const MAX_COLLIDERS: usize = 2_048;
const EPSILON: f64 = 1e-7;
const POSITION_TOLERANCE: f64 = 1e-5;
const HALF_WIDTH: f64 = 0.3;
const STANDING_HEIGHT: f64 = 1.8;
const SNEAKING_HEIGHT: f64 = 1.5;
const UNCERTAIN_POSE_HEIGHT: f64 = 0.6;
const STEP_HEIGHT: f64 = 0.6;
const EDGE_INCREMENT: f64 = 0.05;

#[derive(Clone, Copy, Debug)]
pub struct MovementProposal {
    pub transform: Transform,
    pub sneaking: bool,
    pub sprinting: bool,
}

/// Ordered history matters: replacing a route with its endpoint cuts corners.
#[derive(Default)]
pub struct ProposalQueue {
    pending: VecDeque<MovementProposal>,
    overflowed: bool,
}

impl ProposalQueue {
    /// False requests synchronization at the next simulation step.
    pub fn push(&mut self, proposal: MovementProposal) -> bool {
        if self.overflowed {
            return false;
        }
        if self.pending.len() == PROPOSAL_CAPACITY {
            self.pending.clear();
            self.overflowed = true;
            return false;
        }
        self.pending.push_back(proposal);
        true
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.overflowed = false;
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && !self.overflowed
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Advance {
    pub transform: Transform,
    pub sneaking: bool,
    pub sprinting: bool,
    pub changed: bool,
    pub correction: bool,
    /// Largest clean landing in this bounded batch; unsupported media suppress it.
    pub landed_fall_distance: Option<f64>,
    pub uncertain_motion: bool,
    pub processed: usize,
    pub collision_cells: usize,
}

pub struct MovementState {
    transform: Transform,
    sneaking: bool,
    sprinting: bool,
    uncertain_pose: bool,
    fall_distance: f64,
    uncertain_fall: bool,
    colliders: Vec<Aabb>,
    step_heights: Vec<f64>,
}

impl MovementState {
    pub fn new(transform: Transform) -> Self {
        Self {
            transform,
            sneaking: false,
            sprinting: false,
            uncertain_pose: false,
            fall_distance: 0.0,
            uncertain_fall: false,
            colliders: Vec::with_capacity(64),
            step_heights: Vec::with_capacity(16),
        }
    }

    pub fn transform(&self) -> Transform {
        self.transform
    }

    pub fn sneaking(&self) -> bool {
        self.sneaking
    }

    pub fn sprinting(&self) -> bool {
        self.sprinting
    }

    /// Teleport/respawn establishes a new fall baseline without allocating.
    pub fn reset(&mut self, mut transform: Transform) {
        transform.on_ground = false;
        self.transform = transform;
        self.sneaking = false;
        self.sprinting = false;
        self.uncertain_pose = false;
        self.fall_distance = 0.0;
        self.uncertain_fall = false;
        self.colliders.clear();
        self.step_heights.clear();
    }

    pub fn advance(&mut self, world: &World, proposals: &mut ProposalQueue) -> Advance {
        self.advance_with(world, proposals)
    }

    fn advance_with(&mut self, terrain: &impl Terrain, proposals: &mut ProposalQueue) -> Advance {
        let old_transform = self.transform;
        let old_sneaking = self.sneaking;
        let old_sprinting = self.sprinting;
        let mut result = Advance {
            transform: self.transform,
            sneaking: self.sneaking,
            sprinting: self.sprinting,
            changed: false,
            correction: std::mem::take(&mut proposals.overflowed),
            landed_fall_distance: None,
            uncertain_motion: self.uncertain_fall,
            processed: 0,
            collision_cells: 0,
        };
        for _ in 0..PROPOSALS_PER_STEP {
            if result.correction {
                break;
            }
            let Some(proposal) = proposals.pending.pop_front() else {
                break;
            };
            result.processed += 1;
            if !valid_transform(proposal.transform) {
                result.correction = true;
                break;
            }
            let delta = [
                proposal.transform.x - self.transform.x,
                proposal.transform.y - self.transform.y,
                proposal.transform.z - self.transform.z,
            ];
            // Collect once for ordinary movement, support, uncrouching, and steps.
            // A one-cell rim includes shapes protruding beyond their block cell.
            let standing = player_box(self.transform, false);
            let mut query = expand_towards(standing, delta);
            query.min[1] -= STEP_HEIGHT + POSITION_TOLERANCE;
            query.max[1] += STEP_HEIGHT;
            let contact = inflate(expand_towards(standing, delta), 0.02);
            let Some(uncertain) = collect_colliders(
                terrain,
                query,
                contact,
                &mut self.colliders,
                &mut result.collision_cells,
            ) else {
                result.correction = true;
                break;
            };
            let sneaking = proposal.sneaking
                || (self.sneaking
                    && self.colliders.iter().any(|shape| intersects(standing, *shape)));
            let normal_bounds = player_box(self.transform, sneaking);
            // Unsupported media can produce swimming/crawling poses. A compact
            // envelope preserves body collision without assuming standing height.
            // Keep it through a dry low tunnel until the ordinary body fits.
            let uncertain_pose = uncertain
                || (self.uncertain_pose
                    && self.colliders.iter().any(|shape| intersects(normal_bounds, *shape)));
            let height = if uncertain_pose {
                UNCERTAIN_POSE_HEIGHT
            } else if sneaking {
                SNEAKING_HEIGHT
            } else {
                STANDING_HEIGHT
            };
            let bounds = player_box_with_height(self.transform, height);
            let mut requested = delta;
            if sneaking
                && delta[1] <= 0.0
                && (self.transform.on_ground || supported(bounds, STEP_HEIGHT, &self.colliders))
                && !back_off_edges(bounds, &mut requested, &self.colliders)
            {
                result.correction = true;
                break;
            }
            let mut resolved = collide(bounds, requested, &self.colliders);
            if (self.transform.on_ground || (requested[1] < 0.0 && resolved[1] > requested[1]))
                && horizontal_distance(resolved) + EPSILON < horizontal_distance(requested)
            {
                resolved =
                    try_step(bounds, requested, resolved, &self.colliders, &mut self.step_heights);
            }
            let mut accepted = proposal.transform;
            accepted.x = self.transform.x + resolved[0];
            accepted.y = self.transform.y + resolved[1];
            accepted.z = self.transform.z + resolved[2];
            accepted.on_ground = supported(
                player_box_with_height(accepted, height),
                POSITION_TOLERANCE,
                &self.colliders,
            );
            if uncertain || uncertain_pose {
                self.uncertain_fall = true;
                self.fall_distance = 0.0;
            } else if resolved[1] < 0.0 {
                self.fall_distance -= resolved[1];
            } else if resolved[1] > 0.0 {
                self.fall_distance = 0.0;
            }
            result.uncertain_motion |= uncertain || uncertain_pose || self.uncertain_fall;
            if accepted.on_ground {
                if !self.uncertain_fall && self.fall_distance > 0.0 {
                    result.landed_fall_distance =
                        Some(result.landed_fall_distance.unwrap_or(0.0).max(self.fall_distance));
                }
                self.fall_distance = 0.0;
                // A clean supported landing establishes the next fall baseline.
                self.uncertain_fall = uncertain || uncertain_pose;
            }
            self.transform = accepted;
            self.sneaking = sneaking;
            self.sprinting = proposal.sprinting;
            self.uncertain_pose = uncertain_pose;
            result.correction = delta
                .iter()
                .zip(resolved)
                .any(|(wanted, actual)| (wanted - actual).abs() > POSITION_TOLERANCE);
        }
        if result.correction {
            proposals.clear();
        }
        result.transform = self.transform;
        result.sneaking = self.sneaking;
        result.sprinting = self.sprinting;
        result.changed = self.transform != old_transform
            || self.sneaking != old_sneaking
            || self.sprinting != old_sprinting;
        result
    }
}

fn valid_transform(value: Transform) -> bool {
    value.x.is_finite()
        && value.y.is_finite()
        && value.z.is_finite()
        && value.yaw.is_finite()
        && value.pitch.is_finite()
        && value.x.abs() <= 30_000_000.0
        && value.z.abs() <= 30_000_000.0
        && value.y.abs() <= 20_000_000.0
}

struct TerrainCell {
    shapes: &'static [Aabb],
    uncertain: bool,
    contextual: bool,
}

trait Terrain {
    fn contains_column(&self, x: i32, z: i32) -> bool;
    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell>;
}

impl Terrain for World {
    fn contains_column(&self, x: i32, z: i32) -> bool {
        self.get(x, z).is_some()
    }

    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell> {
        let info = state_info(self.block_at(x, y, z)?);
        Some(TerrainCell {
            shapes: info.collision_boxes(),
            uncertain: info.uncertain_motion(),
            contextual: info.contextual_geometry(),
        })
    }
}

/// None signals unknown terrain or exhausted work, never traversable air.
fn collect_colliders(
    terrain: &impl Terrain,
    query: Aabb,
    contact: Aabb,
    colliders: &mut Vec<Aabb>,
    step_cells: &mut usize,
) -> Option<bool> {
    colliders.clear();
    let min = query.min.map(|n| n.floor() as i32 - 1);
    let max = query.max.map(|n| n.floor() as i32 + 1);
    let mut count = 1usize;
    for axis in 0..3 {
        count = count.checked_mul((i64::from(max[axis]) - i64::from(min[axis]) + 1) as usize)?;
    }
    if count > CELLS_PER_PROPOSAL || count > CELLS_PER_STEP.saturating_sub(*step_cells) {
        return None;
    }
    let mut uncertain = false;
    for x in min[0]..=max[0] {
        for z in min[2]..=max[2] {
            let column_on_path = f64::from(x + 1) > query.min[0]
                && f64::from(x) < query.max[0]
                && f64::from(z + 1) > query.min[2]
                && f64::from(z) < query.max[2];
            if column_on_path
                && query.max[1] > f64::from(MIN_BLOCK_Y)
                && query.min[1] < f64::from(MAX_BLOCK_Y)
                && !terrain.contains_column(x >> 4, z >> 4)
            {
                return None;
            }
            for y in min[1]..=max[1] {
                *step_cells += 1;
                let Some(cell) = terrain.cell(x, y, z) else { continue };
                let origin = [f64::from(x), f64::from(y), f64::from(z)];
                let block = Aabb { min: origin, max: origin.map(|n| n + 1.0) };
                uncertain |= (cell.uncertain || cell.contextual) && intersects(contact, block);
                if !cell.contextual {
                    if cell.shapes.len() > MAX_COLLIDERS.saturating_sub(colliders.len()) {
                        return None;
                    }
                    colliders.extend(cell.shapes.iter().map(|shape| translate(*shape, origin)));
                }
            }
        }
    }
    Some(uncertain)
}

fn player_box(position: Transform, sneaking: bool) -> Aabb {
    player_box_with_height(position, if sneaking { SNEAKING_HEIGHT } else { STANDING_HEIGHT })
}

fn player_box_with_height(position: Transform, height: f64) -> Aabb {
    Aabb {
        min: [position.x - HALF_WIDTH, position.y, position.z - HALF_WIDTH],
        max: [position.x + HALF_WIDTH, position.y + height, position.z + HALF_WIDTH],
    }
}

fn translate(mut bounds: Aabb, delta: [f64; 3]) -> Aabb {
    for (axis, value) in delta.into_iter().enumerate() {
        bounds.min[axis] += value;
        bounds.max[axis] += value;
    }
    bounds
}

fn expand_towards(mut bounds: Aabb, delta: [f64; 3]) -> Aabb {
    for (axis, value) in delta.into_iter().enumerate() {
        bounds.min[axis] += value.min(0.0);
        bounds.max[axis] += value.max(0.0);
    }
    bounds
}

fn inflate(mut bounds: Aabb, amount: f64) -> Aabb {
    for axis in 0..3 {
        bounds.min[axis] -= amount;
        bounds.max[axis] += amount;
    }
    bounds
}

fn intersects(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|axis| a.max[axis] > b.min[axis] + EPSILON && a.min[axis] < b.max[axis] - EPSILON)
}

fn supported(bounds: Aabb, depth: f64, colliders: &[Aabb]) -> bool {
    let feet = Aabb {
        min: [bounds.min[0] + EPSILON, bounds.min[1] - depth - EPSILON, bounds.min[2] + EPSILON],
        max: [bounds.max[0] - EPSILON, bounds.min[1] + EPSILON * 2.0, bounds.max[2] - EPSILON],
    };
    colliders.iter().any(|shape| intersects(feet, *shape))
}

fn collide(mut bounds: Aabb, mut delta: [f64; 3], colliders: &[Aabb]) -> [f64; 3] {
    let axes = if delta[0].abs() < delta[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    for axis in axes {
        if delta[axis] == 0.0 {
            continue;
        }
        for shape in colliders {
            if (0..3).any(|other| {
                other != axis
                    && (bounds.max[other] <= shape.min[other] + EPSILON
                        || bounds.min[other] >= shape.max[other] - EPSILON)
            }) {
                continue;
            }
            if delta[axis] > 0.0 && bounds.max[axis] <= shape.min[axis] + EPSILON {
                delta[axis] = delta[axis].min((shape.min[axis] - bounds.max[axis]).max(0.0));
            } else if delta[axis] < 0.0 && bounds.min[axis] >= shape.max[axis] - EPSILON {
                delta[axis] = delta[axis].max((shape.max[axis] - bounds.min[axis]).min(0.0));
            }
        }
        bounds.min[axis] += delta[axis];
        bounds.max[axis] += delta[axis];
    }
    delta
}

fn horizontal_distance(delta: [f64; 3]) -> f64 {
    delta[0] * delta[0] + delta[2] * delta[2]
}

fn try_step(
    bounds: Aabb,
    requested: [f64; 3],
    ordinary: [f64; 3],
    colliders: &[Aabb],
    heights: &mut Vec<f64>,
) -> [f64; 3] {
    heights.clear();
    for shape in colliders {
        let height = shape.max[1] - bounds.min[1];
        if height > EPSILON && height <= STEP_HEIGHT + EPSILON {
            heights.push(height);
        }
    }
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup_by(|a, b| (*a - *b).abs() <= EPSILON);
    for &height in heights.iter() {
        let candidate = collide(bounds, [requested[0], height, requested[2]], colliders);
        if horizontal_distance(candidate) > horizontal_distance(ordinary) + EPSILON {
            return candidate;
        }
    }
    ordinary
}

fn back_off_edges(bounds: Aabb, delta: &mut [f64; 3], colliders: &[Aabb]) -> bool {
    // The vanilla 0.05 decrement is bounded independently of client coordinates.
    let mut checks = 0;
    for axes in [[true, false], [false, true], [true, true]] {
        loop {
            let x = if axes[0] { delta[0] } else { 0.0 };
            let z = if axes[1] { delta[2] } else { 0.0 };
            if (axes[0] && x == 0.0) || (axes[1] && z == 0.0) {
                break;
            }
            checks += 1;
            if checks > 128 {
                return false;
            }
            if supported(translate(bounds, [x, 0.0, z]), STEP_HEIGHT, colliders) {
                break;
            }
            for axis in [0, 2] {
                if axes[usize::from(axis == 2)] {
                    delta[axis] = if delta[axis].abs() <= EDGE_INCREMENT {
                        0.0
                    } else {
                        delta[axis] - EDGE_INCREMENT.copysign(delta[axis])
                    };
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const FULL: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0; 3] }];
    const SLAB: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }];

    #[derive(Default)]
    struct Map {
        blocks: HashMap<(i32, i32, i32), (&'static [Aabb], bool)>,
        missing: bool,
    }

    impl Map {
        fn floor() -> Self {
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
        assert!(state.advance_with(&map, &mut queue).sneaking);
        propose(&mut queue, position(0.5, 1.0, 0.5));
        assert!(state.advance_with(&map, &mut queue).sneaking);

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
        assert!(
            state.advance_with(&Map { missing: true, ..Map::default() }, &mut queue).correction
        );
        propose(&mut queue, position(f64::NAN, 1.0, -0.5));
        assert!(state.advance_with(&Map::floor(), &mut queue).correction);
    }

    #[test]
    fn accepted_descents_emit_clean_falls_and_media_suppress_the_whole_fall() {
        let mut state = MovementState::new(position(0.5, 5.0, 0.5));
        let mut queue = ProposalQueue::default();
        propose(&mut queue, position(0.5, 3.0, 0.5));
        propose(&mut queue, position(0.5, 1.0, 0.5));
        let result = state.advance_with(&Map::floor(), &mut queue);
        assert!(!result.correction);
        assert_eq!(result.landed_fall_distance, Some(4.0));

        let mut map = Map::floor();
        map.blocks.insert((0, 3, 0), (&[], true));
        state.reset(position(0.5, 5.0, 0.5));
        propose(&mut queue, position(0.5, 3.0, 0.5));
        assert!(state.advance_with(&map, &mut queue).uncertain_motion);
        map.blocks.remove(&(0, 3, 0));
        propose(&mut queue, position(0.5, 1.0, 0.5));
        assert_eq!(state.advance_with(&map, &mut queue).landed_fall_distance, None);
        propose(&mut queue, position(0.5, 5.0, 0.5));
        assert!(!state.advance_with(&map, &mut queue).correction);
        propose(&mut queue, position(0.5, 1.0, 0.5));
        assert_eq!(state.advance_with(&map, &mut queue).landed_fall_distance, Some(4.0));
    }

    #[test]
    fn unsupported_media_allow_low_tunnels_but_retain_body_collision() {
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
        assert!(!result.correction);
        assert!(result.uncertain_motion);
        propose(&mut queue, position(1.5, 1.0, 0.5));
        assert!(!state.advance_with(&map, &mut queue).correction);
        propose(&mut queue, position(2.5, 1.0, 0.5));
        assert!(!state.advance_with(&map, &mut queue).correction);

        // Even the compact envelope cannot enter an ordinary full block.
        map.blocks.insert((3, 1, 0), (FULL, false));
        propose(&mut queue, position(3.5, 1.0, 0.5));
        let result = state.advance_with(&map, &mut queue);
        assert!(result.correction);
        assert!((result.transform.x - 2.7).abs() < EPSILON);

        // Leaving the low tunnel restores normal height on the next proposal.
        map.blocks.remove(&(3, 1, 0));
        propose(&mut queue, position(3.5, 1.0, 0.5));
        assert!(!state.advance_with(&map, &mut queue).correction);
        propose(&mut queue, position(3.5, 1.0, 0.5));
        assert!(!state.advance_with(&map, &mut queue).correction);
        assert!(!state.uncertain_pose);
    }
}
