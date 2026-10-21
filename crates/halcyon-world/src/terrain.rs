//! Immutable terrain queries and the shared, versioned collision catalog.

use std::mem::size_of;

#[path = "collision_data.rs"]
mod data;
#[path = "falling_data.rs"]
mod falling_data;
#[path = "motion_data.rs"]
mod motion_data;

#[derive(Clone, Copy)]
pub struct BlockProperties(u8);

impl BlockProperties {
    pub fn free(self) -> bool {
        self.0 & 1 != 0
    }
    pub fn replaceable(self) -> bool {
        self.0 & 2 != 0
    }
    pub fn suffocating(self) -> bool {
        self.0 & 4 != 0
    }
    pub fn falls(self) -> bool {
        self.0 & 8 != 0
    }
    pub fn entity_motion_effects(self) -> bool {
        self.0 & 16 != 0
    }
}

pub fn block_properties(id: BlockStateId) -> BlockProperties {
    BlockProperties(falling_data::properties(id))
}

pub use data::{COLLISION_MAX_OFFSET, COLLISION_MIN_OFFSET};

pub type BlockStateId = u16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    #[default]
    Ordinary,
    Slime,
    Honey,
    Hay,
    Unsupported,
}

/// Shared block properties, in vanilla's per-50-ms movement units. These are
/// deliberately independent of item effects, attributes, and mutable terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionInfo {
    pub friction: f32,
    pub speed: f32,
    pub jump: f32,
    pub bounce: f32,
    pub fall_reduction: f32,
    pub surface: Surface,
}

impl Default for MotionInfo {
    fn default() -> Self {
        Self {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }
    }
}

/// Special dry surfaces are enabled only with authoritative static geometry.
/// Other uncertain contexts (fluids, climbables, webs, dynamic shapes) cannot
/// acquire support merely because their representative shape is empty.
pub fn motion_info(id: BlockStateId) -> MotionInfo {
    let state = state_info(id);
    if state.flags.has_fluid() || state.contextual_geometry() {
        return MotionInfo { surface: Surface::Unsupported, ..MotionInfo::default() };
    }
    motion_data::special_motion(id).unwrap_or_else(|| MotionInfo {
        surface: if state.uncertain_motion() {
            Surface::Unsupported
        } else {
            Surface::Ordinary
        },
        ..MotionInfo::default()
    })
}

pub const MIN_BLOCK_Y: i32 = -64;
pub const MAX_BLOCK_Y: i32 = 320;
pub const SECTION_COUNT: usize = ((MAX_BLOCK_Y - MIN_BLOCK_Y) / 16) as usize;

/// Local block-space coordinates. Shapes can extend outside the unit cube.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockKind {
    Empty,
    Full,
    Slab,
    Stairs,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct TerrainFlags(u8);

impl TerrainFlags {
    pub const fn has_fluid(self) -> bool {
        self.0 & 1 != 0
    }

    pub const fn uncertain_motion(self) -> bool {
        self.0 & 2 != 0
    }

    pub const fn contextual_geometry(self) -> bool {
        self.0 & 4 != 0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StateInfo {
    shape_id: u16,
    pub kind: BlockKind,
    pub flags: TerrainFlags,
}

impl StateInfo {
    /// Static shapes are authoritative. Contextual shapes are representative
    /// only: callers must check `contextual_geometry` before using them to
    /// reject motion or infer ground support.
    #[inline]
    pub fn collision_boxes(&self) -> &'static [Aabb] {
        let range = data::SHAPES[self.shape_id as usize];
        &data::BOXES[range.start as usize..range.start as usize + range.len as usize]
    }

    pub const fn uncertain_motion(&self) -> bool {
        self.flags.uncertain_motion()
    }

    pub const fn contextual_geometry(&self) -> bool {
        self.flags.contextual_geometry()
    }
}

#[derive(Clone, Copy)]
struct ShapeRange {
    start: u32,
    len: u16,
}

/// IDs originate from the validated loader or the versioned state table.
/// Invalid IDs return `None`; they must never silently become solid or air.
#[inline]
pub fn try_state_info(id: BlockStateId) -> Option<&'static StateInfo> {
    data::STATES.get(id as usize)
}

#[inline]
pub fn state_info(id: BlockStateId) -> &'static StateInfo {
    try_state_info(id).expect("block state ID belongs to the target registry")
}

/// Shared catalog payload, counted once across all worlds. Excludes text keys,
/// executable code, allocator overhead, and OS resident-memory measurements.
pub fn collision_catalog_bytes() -> usize {
    size_of_val(data::STATES) + size_of_val(data::SHAPES) + size_of_val(data::BOXES)
}

/// Uniform sections retain one state, dense sections use an eight-KiB direct
/// array. No palette decoding, allocation, or reference counting per query.
#[derive(Clone, Debug)]
pub enum Section {
    Uniform(BlockStateId),
    Dense(Box<[BlockStateId; 4096]>),
}

impl Default for Section {
    fn default() -> Self {
        Self::Uniform(0)
    }
}

impl Section {
    pub fn from_states(states: Box<[BlockStateId; 4096]>) -> Self {
        let first = states[0];
        if states.iter().all(|&state| state == first) {
            Self::Uniform(first)
        } else {
            Self::Dense(states)
        }
    }

    #[inline]
    pub fn block_at(&self, x: usize, y: usize, z: usize) -> BlockStateId {
        debug_assert!(x < 16 && y < 16 && z < 16);
        match self {
            Self::Uniform(state) => *state,
            Self::Dense(states) => states[(y << 8) | (z << 4) | x],
        }
    }

    pub fn backing_bytes(&self) -> usize {
        match self {
            Self::Uniform(_) => 0,
            Self::Dense(_) => size_of::<[BlockStateId; 4096]>(),
        }
    }
}

#[cfg(test)]
mod tests;
