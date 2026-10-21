//! Bounded collision queries and dry-surface contact rules.

use halcyon_world::{
    Aabb, BlockView, MAX_BLOCK_Y, MIN_BLOCK_Y, MotionInfo, Surface, motion_info, state_info,
};

use super::collision::{intersects, player_box, translate};
use super::{CELLS_PER_PROPOSAL, CELLS_PER_STEP, EPSILON, HALF_WIDTH, MAX_COLLIDERS};
use crate::lobby::Transform;

#[derive(Clone, Copy)]
pub(super) struct GroundContact {
    pub(super) block: [i32; 3],
    pub(super) motion: MotionInfo,
}

/// Vanilla chooses the closest supporting block, preferring the greater
/// (Y, Z, X) position on ties. At most the small foot envelope plus a rim is
/// inspected; protruding static shapes can extend 0.25 horizontally and 1.5 up.
pub(super) fn ground_contact(terrain: &impl Terrain, position: Transform) -> Option<GroundContact> {
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

pub(super) fn landing_damage(distance: f64, surface: MotionInfo, sneaking: bool) -> f32 {
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
pub(super) fn honey_slide(terrain: &impl Terrain, position: Transform, delta_y: f64) -> bool {
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

pub(super) struct TerrainCell {
    pub(super) shapes: &'static [Aabb],
    pub(super) uncertain: bool,
    pub(super) contextual: bool,
    pub(super) motion: MotionInfo,
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
pub(super) fn collect_colliders(
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
