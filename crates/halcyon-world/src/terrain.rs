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
mod tests {
    use super::*;
    use crate::blockstates::block_state_id;

    fn info(key: &str) -> &'static StateInfo {
        state_info(block_state_id(key).unwrap() as BlockStateId)
    }

    #[test]
    fn catalog_matches_versioned_registry() {
        assert_eq!(data::STATES.len(), crate::blockstates::BLOCK_STATES.len());
        assert_eq!(size_of::<StateInfo>(), 4);
        for &(_, id) in crate::blockstates::BLOCK_STATES {
            assert!(try_state_info(id.try_into().unwrap()).is_some());
        }
        for state in data::STATES {
            for shape in state.collision_boxes() {
                for axis in 0..3 {
                    assert!(shape.min[axis].is_finite() && shape.max[axis].is_finite());
                    assert!(shape.min[axis] < shape.max[axis]);
                }
            }
        }
        assert!(try_state_info(u16::MAX).is_none());
    }

    #[test]
    fn static_shapes_and_unsupported_motion_remain_separate() {
        assert!(info("minecraft:air").collision_boxes().is_empty());
        assert_eq!(info("minecraft:stone").kind, BlockKind::Full);
        let slime = info("minecraft:slime_block");
        assert!(slime.uncertain_motion());
        assert!(!slime.contextual_geometry());
        assert_eq!(slime.collision_boxes(), info("minecraft:stone").collision_boxes());
        let water = info("minecraft:water[level=0]");
        assert!(water.flags.has_fluid() && water.uncertain_motion());
        assert!(water.collision_boxes().is_empty());
        let snow = info("minecraft:snow[layers=1]");
        assert!(snow.collision_boxes().is_empty());
        assert_eq!(info("minecraft:snow[layers=8]").collision_boxes()[0].max[1], 0.875);
        assert!(info("minecraft:powder_snow").contextual_geometry());
    }

    #[test]
    fn ground_profiles_match_target_surface_properties_and_keep_media_disabled() {
        let motion = |key| motion_info(block_state_id(key).unwrap() as BlockStateId);
        assert_eq!(motion("minecraft:ice").friction, 0.98);
        assert_eq!(motion("minecraft:blue_ice").friction, 0.989);
        assert_eq!(motion("minecraft:soul_sand").speed, 0.4);
        assert_eq!(motion("minecraft:honey_block").jump, 0.5);
        assert_eq!(motion("minecraft:slime_block").bounce, 1.0);
        for key in ["minecraft:sand", "minecraft:red_sand", "minecraft:gravel"] {
            let properties = block_properties(block_state_id(key).unwrap() as u16);
            assert!(properties.falls() && properties.suffocating());
            assert!(!properties.free() && !properties.replaceable());
        }
        assert!(block_properties(1).suffocating());
        assert!(!block_properties(block_state_id("minecraft:glass").unwrap() as u16).suffocating());
        assert_eq!(motion("minecraft:shelf_mushroom[age=0,facing=north]").bounce, 0.75);
        assert_eq!(
            motion("minecraft:small_dripleaf[facing=west,half=lower,waterlogged=false]"),
            MotionInfo::default()
        );
        assert_eq!(
            motion("minecraft:small_dripleaf[facing=west,half=lower,waterlogged=true]").surface,
            Surface::Unsupported
        );
        for key in [
            "minecraft:ice",
            "minecraft:soul_sand",
            "minecraft:slime_block",
            "minecraft:honey_block",
            "minecraft:mud",
        ] {
            assert_ne!(motion(key).surface, Surface::Unsupported, "{key}");
        }
        for key in [
            "minecraft:water[level=0]",
            "minecraft:cobweb",
            "minecraft:powder_snow",
            "minecraft:ladder[facing=north,waterlogged=false]",
        ] {
            assert_eq!(motion(key).surface, Surface::Unsupported, "{key}");
        }
    }

    #[test]
    fn slabs_stairs_trapdoors_and_tall_shapes_keep_exact_geometry() {
        for (slab_type, min, max) in [("bottom", 0.0, 0.5), ("top", 0.5, 1.0), ("double", 0.0, 1.0)]
        {
            let slab = info(&format!("minecraft:oak_slab[type={slab_type},waterlogged=false]"));
            assert_eq!(slab.kind, BlockKind::Slab);
            assert_eq!(
                slab.collision_boxes(),
                &[Aabb { min: [0.0, min, 0.0], max: [1.0, max, 1.0] }]
            );
        }
        for (facing, min, max) in [
            ("north", [0.0, 0.0, 0.8125], [1.0; 3]),
            ("south", [0.0; 3], [1.0, 1.0, 0.1875]),
            ("east", [0.0; 3], [0.1875, 1.0, 1.0]),
            ("west", [0.8125, 0.0, 0.0], [1.0; 3]),
        ] {
            for (half, height) in [("bottom", 0.0), ("top", 0.8125)] {
                let open = info(&format!(
                    "minecraft:spruce_trapdoor[facing={facing},half={half},open=true,powered=false,waterlogged=false]"
                ));
                assert_eq!(open.collision_boxes(), &[Aabb { min, max }]);
                let closed = info(&format!(
                    "minecraft:spruce_trapdoor[facing={facing},half={half},open=false,powered=false,waterlogged=false]"
                ));
                assert_eq!(
                    closed.collision_boxes(),
                    &[Aabb { min: [0.0, height, 0.0], max: [1.0, height + 0.1875, 1.0] }]
                );
            }
        }
        for facing in ["north", "south", "east", "west"] {
            for half in ["top", "bottom"] {
                for shape in ["straight", "inner_left", "inner_right", "outer_left", "outer_right"]
                {
                    let stair = info(&format!(
                        "minecraft:oak_stairs[facing={facing},half={half},shape={shape},waterlogged=false]"
                    ));
                    assert_eq!(stair.kind, BlockKind::Stairs);
                    assert!(!stair.contextual_geometry());
                    assert!(stair.collision_boxes().len() >= 2);
                    let wet = info(&format!(
                        "minecraft:oak_stairs[facing={facing},half={half},shape={shape},waterlogged=true]"
                    ));
                    assert!(wet.uncertain_motion());
                    assert_eq!(stair.collision_boxes(), wet.collision_boxes());
                }
            }
        }
        let fence = info(
            "minecraft:oak_fence[east=false,north=false,south=false,waterlogged=false,west=false]",
        );
        assert!(fence.collision_boxes().iter().any(|shape| shape.max[1] == 1.5));
    }

    #[test]
    fn sections_retain_y_z_x_order_without_uniform_allocations() {
        assert_eq!(Section::Uniform(1).backing_bytes(), 0);
        let mut blocks = Box::new([0; 4096]);
        blocks[(3 << 8) | (5 << 4) | 7] = 42;
        let section = Section::from_states(blocks);
        assert_eq!(section.block_at(7, 3, 5), 42);
        assert_eq!(section.block_at(3, 7, 5), 0);
        assert_eq!(section.backing_bytes(), 8192);
        assert!(matches!(Section::from_states(Box::new([1; 4096])), Section::Uniform(1)));
    }
}
