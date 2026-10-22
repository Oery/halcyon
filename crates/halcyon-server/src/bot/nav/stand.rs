//! Where a player's feet can rest. A node is the block cell containing the
//! feet; support may come from that cell (slabs, carpets, stair steps) or the
//! block below it.

use halcyon_world::{
    BlockStateId, BlockView, FluidKind, Surface, climb, collision_offset, contextual_geometry,
    fluid_info, motion_info, state_info,
};

use super::Cell;
use crate::lobby::Transform;
use crate::movement::MovementState;

const EPSILON: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandKind {
    Ground,
    /// Facing index (north, east, south, west) of the ladder, away from its wall.
    Ladder(u8),
    /// Vines of any kind: climbable, without a facing.
    Vine,
}

impl StandKind {
    pub fn climbable(self) -> bool {
        matches!(self, StandKind::Ladder(_) | StandKind::Vine)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    pub feet_y: f64,
    pub kind: StandKind,
    /// Block-local horizontal extent of the support under the centre:
    /// `[min_x, max_x, min_z, max_z]`.
    pub extent: [f32; 4],
}

impl Stand {
    /// Distance from the cell centre to the support's rim along `dir`.
    pub fn rim(&self, dir: [f64; 2]) -> f64 {
        let [min_x, max_x, min_z, max_z] = self.extent.map(f64::from);
        let along = |d: f64, min: f64, max: f64| {
            if d > EPSILON {
                (max - 0.5) / d
            } else if d < -EPSILON {
                (0.5 - min) / -d
            } else {
                f64::INFINITY
            }
        };
        along(dir[0], min_x, max_x).min(along(dir[1], min_z, max_z))
    }
}

pub fn centre(cell: Cell) -> [f64; 2] {
    [f64::from(cell[0]) + 0.5, f64::from(cell[2]) + 0.5]
}

/// Collision tops covering the cell centre, ignoring shapes that only touch it.
fn support(id: BlockStateId, x: i32, z: i32) -> Option<(f64, [f32; 4])> {
    let offset = collision_offset(id, x, z);
    let mut top: Option<f64> = None;
    let mut extent = [1.0f32, 0.0, 1.0, 0.0];
    for shape in state_info(id).collision_boxes() {
        let [min_x, max_x] = [shape.min[0] + offset[0], shape.max[0] + offset[0]];
        let [min_z, max_z] = [shape.min[2] + offset[2], shape.max[2] + offset[2]];
        if min_x < 0.5 - EPSILON
            && max_x > 0.5 + EPSILON
            && min_z < 0.5 - EPSILON
            && max_z > 0.5 + EPSILON
        {
            top = Some(top.map_or(shape.max[1], |top: f64| top.max(shape.max[1])));
            extent = [
                extent[0].min(min_x as f32),
                extent[1].max(max_x as f32),
                extent[2].min(min_z as f32),
                extent[3].max(max_z as f32),
            ];
        }
    }
    top.map(|top| (top, extent))
}

fn hazardous(id: BlockStateId) -> bool {
    motion_info(id).surface == Surface::Unsupported
        || contextual_geometry(id)
        || fluid_info(id).kind == FluidKind::Lava
}

/// `probe` is scratch for the clearance check; its state is overwritten.
pub fn stand(view: &impl BlockView, probe: &mut MovementState, cell: Cell) -> Option<Stand> {
    let [x, y, z] = cell;
    let here = view.block_at(x, y, z)?;
    if hazardous(here) || view.block_at(x, y + 1, z).is_some_and(hazardous) {
        return None;
    }
    if fluid_info(here).kind != FluidKind::Empty {
        // Swimming routes are not planned yet.
        return None;
    }
    let climbable = match climb(here) {
        halcyon_world::Climb::Ladder(facing) => Some(StandKind::Ladder(facing)),
        halcyon_world::Climb::Vine => Some(StandKind::Vine),
        _ => None,
    };
    let mut found = if let Some(kind) = climbable {
        Some(Stand { feet_y: f64::from(y), kind, extent: [0.0, 1.0, 0.0, 1.0] })
    } else if let Some((top, extent)) = support(here, x, z) {
        // A full-height shape here is a wall, not a floor.
        (top < 1.0 - EPSILON).then(|| Stand {
            feet_y: f64::from(y) + top,
            kind: StandKind::Ground,
            extent,
        })
    } else {
        None
    };
    if found.is_none() && state_info(here).collision_boxes().is_empty() {
        let below = view.block_at(x, y - 1, z)?;
        if hazardous(below) {
            return None;
        }
        if let Some((top, extent)) = support(below, x, z)
            && (1.0 - EPSILON..2.0).contains(&top)
        {
            found = Some(Stand { feet_y: f64::from(y - 1) + top, kind: StandKind::Ground, extent });
        }
    }
    let stand = found?;
    if stand.feet_y >= f64::from(y) + 1.0 {
        return None;
    }
    let [cx, cz] = centre(cell);
    let at = Transform { x: cx, y: stand.feet_y, z: cz, ..Transform::default() };
    probe.supports_destination(view, at).then_some(stand)
}

/// The node containing a player's feet, if any.
pub fn cell_of(position: Transform) -> Cell {
    [position.x.floor() as i32, (position.y + 1e-3).floor() as i32, position.z.floor() as i32]
}
