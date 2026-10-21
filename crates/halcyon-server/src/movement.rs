//! Shared, bounded ground collision for predictions and server-driven motion.
//!
//! Client coordinates remain proposals checked against geometry and stance;
//! owner-driven inputs use the same resolver with fixed-step ground physics.
//! Human movement does not enforce speed, flight or jump trajectories.
use std::collections::VecDeque;

use halcyon_world::{
    Aabb, BlockView, MAX_BLOCK_Y, MIN_BLOCK_Y, MotionInfo, Surface, motion_info, state_info,
};

use crate::lobby::Transform;

mod physics;
#[cfg(test)]
pub use physics::shared_motion_bytes;
pub use physics::{GroundInput, MotionState};

const PROPOSAL_CAPACITY: usize = 32;
const PROPOSALS_PER_STEP: usize = 4;
const CELLS_PER_PROPOSAL: usize = 256;
const CELLS_PER_STEP: usize = 512;
const MAX_COLLIDERS: usize = 2_048;
const EPSILON: f64 = 1e-7;
const POSITION_TOLERANCE: f64 = 1e-5;
const HALF_WIDTH: f64 = 0.6f32 as f64 / 2.0;
const STANDING_HEIGHT: f64 = 1.8f32 as f64;
const SNEAKING_HEIGHT: f64 = 1.5;
const STEP_HEIGHT: f64 = 0.6f32 as f64;
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

    #[cfg(test)]
    pub(crate) fn capacity_payload_bytes(&self) -> usize {
        self.pending.capacity() * std::mem::size_of::<MovementProposal>()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Advance {
    pub transform: Transform,
    pub sneaking: bool,
    pub crouching: bool,
    pub sprinting: bool,
    pub changed: bool,
    pub correction: bool,
    /// Largest landing damage in this bounded batch, after surface modifiers.
    pub landed_fall_damage: Option<f32>,
    pub uncertain_motion: bool,
    pub processed: usize,
    pub collision_cells: usize,
    /// Actual clipping, independent of whether a client correction is needed.
    pub collisions: [bool; 3],
    pub ground: MotionInfo,
}

pub struct MovementState {
    transform: Transform,
    sneaking: bool,
    crouching: bool,
    sprinting: bool,
    fall_distance: f64,
    pub motion: MotionState,
    colliders: Vec<Aabb>,
    step_heights: Vec<f64>,
}

/// Cold-path recovery for an imported spawn obstructed by map geometry. Search
/// only nearby static support surfaces; do not generate terrain or edit maps.
pub fn resolve_spawn(world: &impl BlockView, requested: Transform) -> Option<Transform> {
    if !requested.is_valid() {
        return None;
    }
    let mut movement = MovementState::new(requested);
    if movement.supports_destination(world, requested) {
        return Some(requested);
    }
    let mut candidates = Vec::new();
    let [sx, sy, sz] = [requested.x, requested.y, requested.z].map(|n| n.floor() as i32);
    for x in sx - 4..=sx + 4 {
        for z in sz - 4..=sz + 4 {
            for y in
                sy.saturating_sub(8).max(MIN_BLOCK_Y)..=sy.saturating_add(8).min(MAX_BLOCK_Y - 1)
            {
                let Some(id) = world.block_at(x, y, z) else { continue };
                if motion_info(id).surface == Surface::Unsupported {
                    continue;
                }
                for shape in state_info(id).collision_boxes() {
                    if shape.min[0] <= 0.5
                        && shape.max[0] >= 0.5
                        && shape.min[2] <= 0.5
                        && shape.max[2] >= 0.5
                    {
                        let destination = Transform {
                            x: f64::from(x) + 0.5,
                            y: f64::from(y) + shape.max[1],
                            z: f64::from(z) + 0.5,
                            on_ground: false,
                            ..requested
                        };
                        let distance = (destination.x - requested.x).powi(2)
                            + (destination.y - requested.y).powi(2)
                            + (destination.z - requested.z).powi(2);
                        candidates.push((distance, destination));
                    }
                }
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    candidates.into_iter().map(|(_, destination)| destination).find(|destination| {
        movement.supports_destination(world, *destination)
            && supported(player_box(*destination, false), POSITION_TOLERANCE, &movement.colliders)
    })
}

impl MovementState {
    #[cfg(test)]
    pub(crate) fn capacity_payload_bytes(&self) -> usize {
        self.colliders.capacity() * std::mem::size_of::<Aabb>()
            + self.step_heights.capacity() * std::mem::size_of::<f64>()
    }
    pub fn new(transform: Transform) -> Self {
        Self {
            transform,
            sneaking: false,
            crouching: false,
            sprinting: false,
            fall_distance: 0.0,
            motion: MotionState::default(),
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

    pub fn crouching(&self) -> bool {
        self.crouching
    }

    pub fn sprinting(&self) -> bool {
        self.sprinting
    }

    /// Vanilla's thin eye-level envelope, rather than the entire player body.
    /// Existing solid overlaps do not forbid movement out of a settled block.
    pub fn suffocating(&self, world: &impl BlockView) -> bool {
        let eye = self.transform.y + f64::from(if self.crouching { 1.27f32 } else { 1.62f32 });
        let radius = f64::from(0.6f32 * 0.8f32) / 2.0;
        let eyes = Aabb {
            min: [self.transform.x - radius, eye - 0.5e-6, self.transform.z - radius],
            max: [self.transform.x + radius, eye + 0.5e-6, self.transform.z + radius],
        };
        for x in eyes.min[0].floor() as i32..=eyes.max[0].floor() as i32 {
            for y in eyes.min[1].floor() as i32..=eyes.max[1].floor() as i32 {
                for z in eyes.min[2].floor() as i32..=eyes.max[2].floor() as i32 {
                    let Some(id) = world.block_at(x, y, z) else {
                        continue;
                    };
                    if halcyon_world::block_properties(id).suffocating()
                        && state_info(id).collision_boxes().iter().any(|shape| {
                            intersects(
                                eyes,
                                translate(*shape, [f64::from(x), f64::from(y), f64::from(z)]),
                            )
                        })
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Teleport/respawn establishes a new fall baseline without allocating.
    pub fn reset(&mut self, mut transform: Transform) {
        transform.on_ground = false;
        self.transform = transform;
        self.sneaking = false;
        self.crouching = false;
        self.sprinting = false;
        self.fall_distance = 0.0;
        self.motion = MotionState::default();
        self.colliders.clear();
        self.step_heights.clear();
    }

    pub fn advance(&mut self, world: &impl BlockView, proposals: &mut ProposalQueue) -> Advance {
        self.advance_with(world, proposals)
    }

    /// Teleports and initial spawning must not bypass supported-content checks.
    pub fn supports_destination(&mut self, world: &impl BlockView, transform: Transform) -> bool {
        if !transform.is_valid() {
            return false;
        }
        let bounds = player_box(transform, false);
        let mut cells = 0;
        collect_colliders(world, bounds, inflate(bounds, 0.02), &mut self.colliders, &mut cells)
            == Some(false)
            && !self.colliders.iter().any(|shape| intersects(bounds, *shape))
    }

    fn advance_with(&mut self, terrain: &impl Terrain, proposals: &mut ProposalQueue) -> Advance {
        let overflowed = std::mem::take(&mut proposals.overflowed);
        let mut result = self.resolve(
            terrain,
            std::iter::from_fn(|| proposals.pending.pop_front()).take(PROPOSALS_PER_STEP),
            true,
            0,
        );
        result.correction |= overflowed;
        if result.correction {
            proposals.clear();
        }
        result
    }

    fn resolve(
        &mut self,
        terrain: &impl Terrain,
        proposals: impl Iterator<Item = MovementProposal>,
        prediction: bool,
        cells: usize,
    ) -> Advance {
        let old_transform = self.transform;
        let old_sneaking = self.sneaking;
        let old_crouching = self.crouching;
        let old_sprinting = self.sprinting;
        let mut result = Advance {
            transform: self.transform,
            sneaking: self.sneaking,
            crouching: self.crouching,
            sprinting: self.sprinting,
            changed: false,
            correction: false,
            landed_fall_damage: None,
            uncertain_motion: false,
            processed: 0,
            collision_cells: cells,
            collisions: [false; 3],
            ground: MotionInfo::default(),
        };
        for proposal in proposals {
            result.processed += 1;
            if !proposal.transform.is_valid() {
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
            let contact = inflate(
                expand_towards(
                    player_box(self.transform, proposal.sneaking || self.crouching),
                    delta,
                ),
                0.02,
            );
            let Some(unsupported) = collect_colliders(
                terrain,
                query,
                contact,
                &mut self.colliders,
                &mut result.collision_cells,
            ) else {
                result.correction = true;
                break;
            };
            if unsupported {
                result.uncertain_motion = true;
                result.correction = true;
                break;
            }
            let crouching = proposal.sneaking
                || (self.crouching
                    && self.colliders.iter().any(|shape| intersects(standing, *shape)));
            let height = if crouching { SNEAKING_HEIGHT } else { STANDING_HEIGHT };
            let bounds = player_box_with_height(self.transform, height);
            let mut requested = delta;
            if proposal.sneaking
                && delta[1] <= 0.0
                && (self.transform.on_ground
                    || (self.fall_distance < STEP_HEIGHT
                        && supported(bounds, STEP_HEIGHT, &self.colliders)))
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
            result.collisions =
                std::array::from_fn(|axis| (requested[axis] - resolved[axis]).abs() > EPSILON);
            if !prediction {
                accepted.on_ground = requested[1] < 0.0 && result.collisions[1];
            }
            let ground = ground_contact(terrain, accepted);
            result.ground = ground.map_or_else(MotionInfo::default, |contact| contact.motion);
            if resolved[1] < 0.0 {
                self.fall_distance -= f64::from(resolved[1] as f32);
            } else if resolved[1] > 0.0 {
                self.fall_distance = 0.0;
            }
            if accepted.on_ground {
                if self.fall_distance > 0.0 {
                    let damage =
                        landing_damage(self.fall_distance, result.ground, proposal.sneaking);
                    result.landed_fall_damage =
                        Some(result.landed_fall_damage.unwrap_or(0.0).max(damage));
                }
                self.fall_distance = 0.0;
            }
            if honey_slide(terrain, accepted, resolved[1]) {
                self.fall_distance = 0.0;
            }
            self.transform = accepted;
            self.sneaking = proposal.sneaking;
            self.crouching = crouching;
            self.sprinting = proposal.sprinting;
            result.correction = prediction
                && delta
                    .iter()
                    .zip(resolved)
                    .any(|(wanted, actual)| (wanted - actual).abs() > POSITION_TOLERANCE);
            if result.correction {
                break;
            }
        }
        result.transform = self.transform;
        result.sneaking = self.sneaking;
        result.crouching = self.crouching;
        result.sprinting = self.sprinting;
        result.changed = self.transform != old_transform
            || self.sneaking != old_sneaking
            || self.crouching != old_crouching
            || self.sprinting != old_sprinting;
        result
    }
}

#[derive(Clone, Copy)]
struct GroundContact {
    block: [i32; 3],
    motion: MotionInfo,
}

/// Vanilla chooses the closest supporting block, preferring the greater
/// (Y, Z, X) position on ties. At most the small foot envelope plus a rim is
/// inspected; protruding static shapes can extend 0.25 horizontally and 1.5 up.
fn ground_contact(terrain: &impl Terrain, position: Transform) -> Option<GroundContact> {
    let bounds = player_box(position, true);
    let feet = Aabb {
        min: [bounds.min[0], position.y - 1e-6, bounds.min[2]],
        max: [bounds.max[0], position.y, bounds.max[2]],
    };
    let mut best: Option<(f64, GroundContact)> = None;
    for y in (position.y - 1.5).floor() as i32..=position.y.floor() as i32 {
        for z in (bounds.min[2] - 0.25).floor() as i32..=(bounds.max[2] + 0.25).floor() as i32 {
            for x in (bounds.min[0] - 0.25).floor() as i32..=(bounds.max[0] + 0.25).floor() as i32 {
                let Some(cell) = terrain.cell(x, y, z) else { continue };
                if cell.contextual || cell.uncertain {
                    continue;
                }
                let origin = [f64::from(x), f64::from(y), f64::from(z)];
                if !cell.shapes.iter().any(|shape| intersects(feet, translate(*shape, origin))) {
                    continue;
                }
                let distance = (position.x - origin[0] - 0.5).powi(2)
                    + (position.y - origin[1] - 0.5).powi(2)
                    + (position.z - origin[2] - 0.5).powi(2);
                let contact = GroundContact { block: [x, y, z], motion: cell.motion };
                if best.is_none_or(|(d, b)| {
                    distance < d
                        || (distance == d && [y, z, x] > [b.block[1], b.block[2], b.block[0]])
                }) {
                    best = Some((distance, contact));
                }
            }
        }
    }
    best.map(|(_, contact)| contact)
}

fn landing_damage(distance: f64, surface: MotionInfo, sneaking: bool) -> f32 {
    if surface.surface == Surface::Slime && !sneaking {
        return 0.0;
    }
    let distance = distance * f64::from(1.0 - surface.fall_reduction);
    let multiplier = if matches!(surface.surface, Surface::Honey | Surface::Hay) {
        f64::from(0.2f32)
    } else {
        1.0
    };
    ((distance - 3.0) * multiplier).ceil().max(0.0) as f32
}

/// Honey's inset collision shape leaves a narrow inside-block contact along
/// its sides. Sliding is not climbing and is supported for dry honey blocks.
fn honey_slide(terrain: &impl Terrain, position: Transform, delta_y: f64) -> bool {
    if position.on_ground || delta_y >= -0.08 {
        return false;
    }
    let bounds = player_box(position, true);
    for x in bounds.min[0].floor() as i32..=bounds.max[0].floor() as i32 {
        for z in bounds.min[2].floor() as i32..=bounds.max[2].floor() as i32 {
            for y in bounds.min[1].floor() as i32..=bounds.max[1].floor() as i32 {
                let Some(cell) = terrain.cell(x, y, z) else { continue };
                if cell.motion.surface == Surface::Honey
                    && position.y <= f64::from(y) + 0.9375 - EPSILON
                    && ((f64::from(x) + 0.5 - position.x).abs() + EPSILON > 0.4375 + HALF_WIDTH
                        || (f64::from(z) + 0.5 - position.z).abs() + EPSILON > 0.4375 + HALF_WIDTH)
                {
                    return true;
                }
            }
        }
    }
    false
}

struct TerrainCell {
    shapes: &'static [Aabb],
    uncertain: bool,
    contextual: bool,
    motion: MotionInfo,
}

trait Terrain {
    fn contains_column(&self, x: i32, z: i32) -> bool;
    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell>;
}

impl<T: BlockView> Terrain for T {
    fn contains_column(&self, x: i32, z: i32) -> bool {
        BlockView::contains_column(self, x, z)
    }

    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell> {
        let id = self.block_at(x, y, z)?;
        let info = state_info(id);
        let motion = motion_info(id);
        Some(TerrainCell {
            shapes: info.collision_boxes(),
            uncertain: motion.surface == Surface::Unsupported,
            contextual: info.contextual_geometry(),
            motion,
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
    let landing = requested[1] < 0.0 && ordinary[1] > requested[1];
    let offset = if landing { ordinary[1] } else { 0.0 };
    let base = translate(bounds, [0.0, offset, 0.0]);
    let mut query = expand_towards(base, [requested[0], STEP_HEIGHT, requested[2]]);
    if !landing {
        query.min[1] -= f64::from(1e-5f32);
    }
    heights.clear();
    for shape in colliders {
        if !intersects(query, *shape) {
            continue;
        }
        for y in [shape.min[1], shape.max[1]] {
            let height = f64::from((y - base.min[1]) as f32);
            if (0.0..=STEP_HEIGHT).contains(&height) && height != f64::from(ordinary[1] as f32) {
                heights.push(height);
            }
        }
    }
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    for &height in heights.iter() {
        let mut candidate = collide(base, [requested[0], height, requested[2]], colliders);
        if horizontal_distance(candidate) > horizontal_distance(ordinary) + EPSILON {
            candidate[1] += offset;
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
mod tests;
