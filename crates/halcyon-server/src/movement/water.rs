//! Local water and lava interaction in the target client's discrete movement
//! units. One pass measures both fluids; every cell, including flow neighbors,
//! is charged to the shared work budget.
use super::*;
use halcyon_world::{FluidInfo, FluidKind};

#[cfg(test)]
#[path = "../tests/tests_movement_water.rs"]
mod tests;

const SIDES: [[i32; 3]; 4] = [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]];

/// Vanilla's overworld lava current scale; Halcyon lobbies do not use fast lava.
pub(super) const LAVA_CURRENT: f64 = 0.0023333333333333335;
pub(super) const WATER_CURRENT: f64 = 0.014;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Fluids {
    pub water: FluidContact,
    pub lava: FluidContact,
    /// A fluid occupies the cell at waist height (any fluid, as in vanilla).
    pub high: bool,
}

impl Fluids {
    pub fn touching(self) -> bool {
        self.water.touching() || self.lava.touching()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FluidContact {
    pub height: f64,
    pub eyes: bool,
    pub feet: bool,
    pub current: [f64; 3],
    count: u32,
}

impl FluidContact {
    pub fn touching(self) -> bool {
        self.height > 0.0
    }

    pub fn push(self, velocity: &mut [f64; 3], scale: f64) {
        let mut push = self.current.map(|n| n * scale);
        let length = length(push);
        if length > 0.0 && velocity[0].abs() < 0.003 && velocity[2].abs() < 0.003 && length < 0.0045
        {
            push = push.map(|n| n / length * 0.0045);
        }
        for axis in 0..3 {
            velocity[axis] += push[axis];
        }
    }
}

fn cell(terrain: &impl Terrain, pos: [i32; 3], cells: &mut usize) -> Option<Option<TerrainCell>> {
    if *cells >= CELLS_PER_STEP {
        return None;
    }
    *cells += 1;
    if (MIN_BLOCK_Y..MAX_BLOCK_Y).contains(&pos[1])
        && !terrain.contains_column(pos[0] >> 4, pos[2] >> 4)
    {
        return None;
    }
    Some(terrain.cell(pos[0], pos[1], pos[2]))
}

pub(super) fn fluid_contact(
    terrain: &impl Terrain,
    position: Transform,
    pose: PlayerPose,
    currents: bool,
    cells: &mut usize,
) -> Option<Fluids> {
    let bounds = inflate(player_box_with_height(position, pose_height(pose)), -0.001);
    let min = bounds.min.map(|n| n.floor() as i32);
    let max = bounds.max.map(|n| n.ceil() as i32 - 1);
    let eye = position.y + eye_height(pose);
    let center = [position.x.floor() as i32, position.y.floor() as i32, position.z.floor() as i32];
    let mut fluids = Fluids::default();
    for x in min[0]..=max[0] {
        for y in min[1]..=max[1] {
            for z in min[2]..=max[2] {
                let pos = [x, y, z];
                let Some(block) = cell(terrain, pos, cells)? else {
                    continue;
                };
                let fluid = block.fluid;
                let contact = match fluid.kind {
                    FluidKind::Empty => continue,
                    FluidKind::Water => &mut fluids.water,
                    FluidKind::Lava => &mut fluids.lava,
                };
                let above = cell(terrain, [x, y + 1, z], cells)?;
                let height = if above.as_ref().is_some_and(|b| b.fluid.kind == fluid.kind) {
                    1.0
                } else {
                    f64::from(fluid.own_height())
                };
                let surface = f64::from(y) + height;
                if surface < bounds.min[1] {
                    continue;
                }
                contact.height = contact.height.max(surface - position.y);
                contact.feet |= pos == center;
                let camera_surface = if fluid.amount == 8
                    && !fluid.falling
                    && above.is_some_and(|block| block.fluid.down_face)
                {
                    f64::from(y + 1)
                } else {
                    surface
                };
                contact.eyes |= x == center[0]
                    && z == center[2]
                    && eye >= f64::from(y)
                    && eye <= camera_surface;
                if currents {
                    let mut flow = flow(terrain, pos, fluid, cells)?;
                    if contact.height < 0.4 {
                        flow = flow.map(|n| n * contact.height);
                    }
                    for (sum, value) in contact.current.iter_mut().zip(flow) {
                        *sum += value;
                    }
                    contact.count += 1;
                }
            }
        }
    }
    for contact in [&mut fluids.water, &mut fluids.lava] {
        if contact.count > 0 && length(contact.current).powi(2) >= f64::from(1e-5f32) {
            contact.current = contact.current.map(|n| n / f64::from(contact.count));
        } else {
            contact.current = [0.0; 3];
        }
    }
    if currents && fluids.touching() {
        fluids.high =
            cell(terrain, [center[0], (position.y + 0.9).floor() as i32, center[2]], cells)?
                .is_some_and(|block| block.fluid.kind != FluidKind::Empty);
    }
    Some(fluids)
}

fn flow(
    terrain: &impl Terrain,
    pos: [i32; 3],
    fluid: FluidInfo,
    cells: &mut usize,
) -> Option<[f64; 3]> {
    let mut flow = [0.0; 3];
    let mut wall = false;
    for (face, side) in SIDES.into_iter().enumerate() {
        let next = std::array::from_fn(|axis| pos[axis] + side[axis]);
        let neighbor = cell(terrain, next, cells)?;
        if neighbor.as_ref().is_some_and(|c| c.contextual) {
            return None;
        }
        let other = neighbor.as_ref().map_or_else(FluidInfo::default, |c| c.fluid);
        if other.kind == FluidKind::Empty || other.kind == fluid.kind {
            let difference = if other.amount > 0 {
                fluid.own_height() - other.own_height()
            } else if !other.blocks_flow {
                let below = cell(terrain, [next[0], next[1] - 1, next[2]], cells)?;
                match below.map(|c| c.fluid) {
                    Some(below) if below.kind == fluid.kind => {
                        fluid.own_height() - (below.own_height() - 0.8888889f32)
                    }
                    _ => 0.0,
                }
            } else {
                0.0
            };
            flow[0] += f64::from(side[0] as f32 * difference);
            flow[2] += f64::from(side[2] as f32 * difference);
        }
        if fluid.falling && !wall {
            let above = cell(terrain, [next[0], next[1] + 1, next[2]], cells)?;
            wall = [Some(other), above.map(|c| c.fluid)]
                .into_iter()
                .flatten()
                .any(|other| other.kind != fluid.kind && other.flow_faces & (1 << face) != 0);
        }
    }
    if fluid.falling && wall {
        flow = normalize(flow);
        flow[1] -= 6.0;
    }
    Some(normalize(flow))
}

fn length(vector: [f64; 3]) -> f64 {
    vector.iter().map(|n| n * n).sum::<f64>().sqrt()
}

fn normalize(vector: [f64; 3]) -> [f64; 3] {
    let length = length(vector);
    if length < 1e-5 {
        [0.0; 3]
    } else {
        vector.map(|n| n / length)
    }
}
