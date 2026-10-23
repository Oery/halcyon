//! Bounded collision queries, pose clearance and supported surface contacts.

use halcyon_world::{
    Aabb, BlockStateId, BlockView, Climb, MAX_BLOCK_Y, MIN_BLOCK_Y, MotionInfo, Surface, climb,
    collision_offset, contextual_geometry, motion_info, state_info, xz_offset,
};

use super::collision::{intersects, player_box, translate};
use super::{CELLS_PER_PROPOSAL, CELLS_PER_STEP, EPSILON, HALF_WIDTH, MAX_COLLIDERS};
use crate::instance::Transform;

#[derive(Clone, Copy)]
pub(super) struct GroundContact {
    pub(super) block: [i32; 3],
    pub(super) motion: MotionInfo,
}

/// Vanilla chooses the closest supporting block, preferring the greater
/// (Y, Z, X) position on ties. At most the small foot envelope plus a rim is
/// inspected; protruding static shapes can extend 0.25 horizontally and 1.5 up.
pub(super) fn ground_contact(
    terrain: &impl Terrain,
    position: Transform,
    context: ShapeContext,
) -> Option<GroundContact> {
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
                let placed = cell.placement(origin);
                if !cell
                    .collision_shapes(context, y)
                    .iter()
                    .any(|shape| intersects(feet, translate(*shape, placed)))
                {
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

/// LivingEntity.onClimbable for the feet cell, including matching open
/// trapdoors directly above ladders.
pub(super) fn climbing(terrain: &impl Terrain, position: Transform) -> bool {
    let [x, y, z] = [position.x, position.y, position.z].map(|n| n.floor() as i32);
    match terrain.cell(x, y, z).map(|cell| cell.climb) {
        Some(Climb::Ladder(_) | Climb::Vine | Climb::Scaffolding) => true,
        Some(Climb::OpenTrapdoor(facing)) => {
            terrain.cell(x, y - 1, z).is_some_and(|cell| cell.climb == Climb::Ladder(facing))
        }
        _ => false,
    }
}

pub(super) fn landing_damage(distance: f64, surface: MotionInfo, sneaking: bool) -> f32 {
    if surface.surface == Surface::PowderSnow {
        return 0.0;
    }
    if surface.surface == Surface::DripstoneTip {
        return ((distance + 2.5 - 3.0) * 2.0).ceil().max(0.0) as f32;
    }
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
pub(super) fn honey_slide(
    terrain: &impl Terrain,
    position: Transform,
    pose: super::PlayerPose,
    delta_y: f64,
) -> bool {
    if position.on_ground || delta_y >= -0.08 {
        return false;
    }
    let bounds = super::player_box_with_height(position, super::pose_height(pose));
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

#[derive(Clone, Copy)]
pub(super) struct TerrainCell {
    pub(super) fluid: halcyon_world::FluidInfo,
    pub(super) shapes: &'static [Aabb],
    /// Position-dependent translation of `shapes` within the block cell.
    pub(super) offset: [f64; 3],
    pub(super) uncertain: bool,
    pub(super) contextual: bool,
    pub(super) motion: MotionInfo,
    pub(super) climb: Climb,
}

impl TerrainCell {
    pub(super) fn placement(&self, origin: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| origin[axis] + self.offset[axis])
    }

    pub(super) fn collision_shapes(&self, context: ShapeContext, y: i32) -> &'static [Aabb] {
        const FULL: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0; 3] }];
        const BOTTOM: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0, 0.125, 1.0] }];
        const SNOW_FALL: &[Aabb] = &[Aabb { min: [0.0; 3], max: [1.0, 0.9f32 as f64, 1.0] }];
        let above = |height: f64| context.feet > f64::from(y) + height - f64::from(1e-5f32);
        match self.motion.surface {
            Surface::Scaffolding { .. } if above(1.0) && !context.descending => self.shapes,
            Surface::Scaffolding { bottom: true } if above(0.0) => BOTTOM,
            Surface::Scaffolding { .. } => &[],
            Surface::PowderSnow if context.fall_distance > 2.5 => SNOW_FALL,
            Surface::PowderSnow if context.powder_walk && !context.descending && above(1.0) => FULL,
            Surface::PowderSnow => &[],
            _ => self.shapes,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct ShapeContext {
    pub(super) feet: f64,
    pub(super) fall_distance: f64,
    pub(super) descending: bool,
    pub(super) powder_walk: bool,
}

pub(super) trait Terrain {
    fn contains_column(&self, x: i32, z: i32) -> bool;
    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell>;
}

impl<T: BlockView> Terrain for T {
    fn contains_column(&self, x: i32, z: i32) -> bool {
        BlockView::contains_column(self, x, z)
    }

    fn cell(&self, x: i32, y: i32, z: i32) -> Option<TerrainCell> {
        let id = self.block_at(x, y, z)?;
        let state = state_cell(id);
        Some(TerrainCell {
            offset: if state.offset {
                collision_offset(id, x, z)
            } else {
                [0.0; 3]
            },
            ..state.cell
        })
    }
}

/// The part of a terrain cell that depends on the block state alone, with
/// whether its shapes are offset by position.
#[derive(Clone, Copy)]
struct StateCell {
    cell: TerrainCell,
    offset: bool,
}

fn derive_state_cell(id: BlockStateId) -> StateCell {
    let motion = motion_info(id);
    StateCell {
        cell: TerrainCell {
            fluid: halcyon_world::fluid_info(id),
            shapes: state_info(id).collision_boxes(),
            offset: [0.0; 3],
            uncertain: motion.surface == Surface::Unsupported,
            contextual: contextual_geometry(id),
            motion,
            climb: climb(id),
        },
        offset: xz_offset(id),
    }
}

/// Physics reads these for every block it touches: derived once per state
/// rather than through the property lookups on every read.
fn state_cell(id: BlockStateId) -> StateCell {
    static CELLS: std::sync::OnceLock<Box<[StateCell]>> = std::sync::OnceLock::new();
    let cells = CELLS.get_or_init(|| {
        let states = halcyon_world::blockstates::BLOCK_STATES;
        let len = states.iter().map(|&(_, id)| id as usize + 1).max().unwrap_or(0);
        (0..len).map(|id| derive_state_cell(id as BlockStateId)).collect()
    });
    cells.get(id as usize).copied().unwrap_or_else(|| derive_state_cell(id))
}

#[derive(Default)]
pub(super) struct CollisionContext {
    pub(super) unsupported: bool,
    pub(super) inside_effects: bool,
}

/// None signals unknown terrain or exhausted work, never traversable air.
pub(super) fn collect_colliders(
    terrain: &impl Terrain,
    query: Aabb,
    contact: Aabb,
    shape_context: ShapeContext,
    colliders: &mut Vec<Aabb>,
    step_cells: &mut usize,
) -> Option<CollisionContext> {
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
    let mut context = CollisionContext::default();
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
                if (cell.uncertain
                    || cell.contextual
                    || super::media::has_inside_effects(cell.motion.surface))
                    && intersects(contact, block)
                {
                    context.unsupported |= cell.uncertain || cell.contextual;
                    context.inside_effects |= super::media::has_inside_effects(cell.motion.surface);
                }
                if !cell.contextual {
                    let shapes = cell.collision_shapes(shape_context, y);
                    if shapes.len() > MAX_COLLIDERS.saturating_sub(colliders.len()) {
                        return None;
                    }
                    let placed = cell.placement(origin);
                    colliders.extend(shapes.iter().map(|shape| translate(*shape, placed)));
                }
            }
        }
    }
    Some(context)
}
