//! Shared, bounded ground/fluid collision and poses for predictions and inputs.
//!
//! Client coordinates remain proposals checked against geometry and stance;
//! owner-driven inputs use the same resolver with fixed-step travel physics.
//! Human movement does not enforce speed, flight or jump trajectories.
use std::collections::VecDeque;

use halcyon_world::{
    Aabb, BlockView, MAX_BLOCK_Y, MIN_BLOCK_Y, MotionInfo, Surface, motion_info, state_info,
};

use crate::lobby::Transform;

mod collision;
mod media;
mod physics;
mod terrain;
mod water;

pub use mc_protocol::packets::client::PlayerPose;
use water::{Fluids, LAVA_CURRENT, WATER_CURRENT, fluid_contact};

use collision::{
    back_off_edges, collide, expand_towards, horizontal_distance, inflate, intersects, player_box,
    player_box_with_height, supported, translate, try_step,
};
use media::{InsideEffects, inside_effects};
#[cfg(test)]
pub use physics::shared_motion_bytes;
pub use physics::{MotionState, MovementInput};
use terrain::{
    ShapeContext, Terrain, TerrainCell, climbing, collect_colliders, ground_contact, honey_slide,
    landing_damage,
};

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
    pub pose: PlayerPose,
    pub swimming: bool,
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

#[derive(Clone)]
pub struct MovementState {
    transform: Transform,
    sneaking: bool,
    pose: PlayerPose,
    swimming: bool,
    sprinting: bool,
    fall_distance: f64,
    powder_walk: bool,
    flying: bool,
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
                let offset = halcyon_world::collision_offset(id, x, z);
                for shape in state_info(id).collision_boxes() {
                    let shape = translate(*shape, offset);
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
            pose: PlayerPose::Standing,
            swimming: false,
            sprinting: false,
            fall_distance: 0.0,
            powder_walk: false,
            flying: false,
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

    pub fn pose(&self) -> PlayerPose {
        self.pose
    }

    pub fn swimming(&self) -> bool {
        self.swimming
    }

    pub fn sprinting(&self) -> bool {
        self.sprinting
    }

    pub fn set_powder_snow_walk(&mut self, enabled: bool) {
        self.powder_walk = enabled;
    }

    pub fn set_flying(&mut self, enabled: bool) {
        if self.flying != enabled {
            self.fall_distance = 0.0;
            self.motion = MotionState::default();
        }
        self.flying = enabled;
    }

    fn shape_context(&self, descending: bool) -> ShapeContext {
        ShapeContext {
            feet: self.transform.y,
            fall_distance: self.fall_distance,
            descending,
            powder_walk: self.powder_walk,
        }
    }

    /// Vanilla's thin eye-level envelope, rather than the entire player body.
    /// Existing solid overlaps do not forbid movement out of a settled block.
    pub fn suffocating(&self, world: &impl BlockView) -> bool {
        let eye = self.transform.y + eye_height(self.pose);
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

    /// Like `reset`, but trusts the caller's ground contact. Offline planners
    /// place a settled player on a known support without simulating a landing.
    pub fn place(&mut self, transform: Transform) {
        self.reset(transform);
        self.transform.on_ground = transform.on_ground;
        if transform.on_ground {
            // A resting player carries one tick of gravity into every step.
            self.motion.velocity[1] = -physics::RESTING_FALL;
        }
    }

    /// Teleport/respawn establishes a new fall baseline without allocating.
    pub fn reset(&mut self, mut transform: Transform) {
        transform.on_ground = false;
        self.transform = transform;
        self.sneaking = false;
        self.pose = PlayerPose::Standing;
        self.swimming = false;
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
        let context =
            ShapeContext { feet: transform.y, powder_walk: self.powder_walk, ..Default::default() };
        collect_colliders(
            world,
            bounds,
            inflate(bounds, 0.02),
            context,
            &mut self.colliders,
            &mut cells,
        )
        .is_some_and(|context| !context.unsupported)
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
        let old_pose = self.pose;
        let old_swimming = self.swimming;
        let old_sprinting = self.sprinting;
        let mut result = Advance {
            transform: self.transform,
            sneaking: self.sneaking,
            pose: self.pose,
            swimming: self.swimming,
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
                    player_box_with_height(self.transform, pose_height(self.pose)),
                    delta,
                ),
                0.02,
            );
            let shape_context = self.shape_context(proposal.sneaking);
            let Some(context) = collect_colliders(
                terrain,
                query,
                contact,
                shape_context,
                &mut self.colliders,
                &mut result.collision_cells,
            ) else {
                result.correction = true;
                break;
            };
            if context.unsupported {
                result.uncertain_motion = true;
                result.correction = true;
                break;
            }
            let Some(Fluids { water, lava, .. }) = fluid_contact(
                terrain,
                self.transform,
                self.pose,
                false,
                &mut result.collision_cells,
            ) else {
                result.correction = true;
                break;
            };
            // Entity.baseTick halves, rather than resets, fall distance in lava;
            // only travel outside fluids resets it on climbable blocks.
            if lava.touching() {
                self.fall_distance *= 0.5;
            } else if !water.touching() && climbing(terrain, self.transform) {
                self.fall_distance = 0.0;
            }
            let swimming = !self.flying
                && proposal.sprinting
                && water.touching()
                && (self.swimming || (water.eyes && water.feet));
            let pose = select_pose(
                self.transform,
                proposal.sneaking && !self.flying,
                swimming,
                &self.colliders,
            );
            let height = pose_height(pose);
            let bounds = player_box_with_height(self.transform, height);
            let mut requested = delta;
            if !self.flying
                && proposal.sneaking
                && pose != PlayerPose::Swimming
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
            let ground = ground_contact(terrain, accepted, shape_context);
            result.ground = ground.map_or_else(MotionInfo::default, |contact| contact.motion);
            let Some(Fluids { water: water_after, .. }) =
                fluid_contact(terrain, accepted, pose, false, &mut result.collision_cells)
            else {
                result.correction = true;
                break;
            };
            let effects = if context.inside_effects {
                let Some(effects) = inside_effects(
                    terrain,
                    bounds,
                    resolved,
                    accepted,
                    shape_context,
                    &mut result.collision_cells,
                ) else {
                    result.correction = true;
                    break;
                };
                effects
            } else {
                InsideEffects::default()
            };
            if self.flying || water.touching() || water_after.touching() {
                self.fall_distance = 0.0;
            } else if resolved[1] < 0.0 {
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
            if honey_slide(terrain, accepted, pose, resolved[1]) {
                self.fall_distance = 0.0;
            }
            // Inside-block effects run after movement and landing. The stuck
            // multiplier is consumed by the next input-driven move; client
            // proposals already include it and must not be slowed a second time.
            if effects.reset_fall {
                self.fall_distance = 0.0;
            }
            self.motion.effects = effects;
            self.transform = accepted;
            self.sneaking = proposal.sneaking;
            self.pose = select_pose(
                accepted,
                proposal.sneaking && !self.flying,
                swimming && water_after.touching(),
                &self.colliders,
            );
            self.swimming = swimming && water_after.touching();
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
        result.pose = self.pose;
        result.swimming = self.swimming;
        result.sprinting = self.sprinting;
        result.changed = self.transform != old_transform
            || self.sneaking != old_sneaking
            || self.pose != old_pose
            || self.swimming != old_swimming
            || self.sprinting != old_sprinting;
        result
    }
}

fn pose_height(pose: PlayerPose) -> f64 {
    match pose {
        PlayerPose::Standing => STANDING_HEIGHT,
        PlayerPose::Crouching => SNEAKING_HEIGHT,
        PlayerPose::Swimming => 0.6f32 as f64,
    }
}

fn eye_height(pose: PlayerPose) -> f64 {
    f64::from(match pose {
        PlayerPose::Standing => 1.62f32,
        PlayerPose::Crouching => 1.27f32,
        PlayerPose::Swimming => 0.4f32,
    })
}

fn select_pose(
    position: Transform,
    sneaking: bool,
    swimming: bool,
    colliders: &[Aabb],
) -> PlayerPose {
    let desired = if swimming {
        PlayerPose::Swimming
    } else if sneaking {
        PlayerPose::Crouching
    } else {
        PlayerPose::Standing
    };
    for pose in [desired, PlayerPose::Crouching, PlayerPose::Swimming] {
        let bounds = player_box_with_height(position, pose_height(pose));
        if !colliders.iter().any(|shape| intersects(bounds, *shape)) {
            return pose;
        }
    }
    desired
}

#[cfg(test)]
#[path = "../tests/tests_movement.rs"]
mod tests;
