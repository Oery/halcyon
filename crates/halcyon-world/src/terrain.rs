//! Immutable terrain queries and the shared, versioned collision catalog.

use std::mem::size_of;
use std::sync::OnceLock;

#[path = "collision_data.rs"]
mod data;
#[path = "falling_data.rs"]
mod falling_data;
#[path = "fluid_data.rs"]
mod fluid_data;
#[path = "motion_data.rs"]
mod motion_data;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FluidKind {
    #[default]
    Empty,
    Water,
    Lava,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FluidInfo {
    pub kind: FluidKind,
    pub amount: u8,
    pub falling: bool,
    /// Fluid support does not enable the block's other movement effects.
    pub supported: bool,
    pub blocks_flow: bool,
    /// Sturdy horizontal faces in target Direction.Plane order: N, E, S, W.
    pub flow_faces: u8,
    pub down_face: bool,
    dry_state: BlockStateId,
}

impl FluidInfo {
    pub fn own_height(self) -> f32 {
        f32::from(self.amount) / 9.0
    }
}

pub fn fluid_info(id: BlockStateId) -> FluidInfo {
    let bits = fluid_data::FLUIDS[id as usize];
    let amount = (bits & 15) as u8;
    FluidInfo {
        kind: if amount == 0 {
            FluidKind::Empty
        } else if bits & 32 == 0 {
            FluidKind::Water
        } else {
            FluidKind::Lava
        },
        amount,
        falling: bits & 16 != 0,
        supported: amount != 0
            && (bits & 64 == 0
                || matches!(id, 18651..=18652 | 24063..=24094 | 33566..=33605 | 33713..=33744)),
        blocks_flow: bits & (1 << 24) != 0,
        flow_faces: ((bits >> 25) & 15) as u8,
        down_face: bits & (1 << 29) != 0,
        dry_state: (bits >> 8) as u16,
    }
}

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
    BerryBush,
    Cobweb,
    PowderSnow,
    Scaffolding {
        bottom: bool,
    },
    BubbleColumn {
        down: bool,
    },
    DripstoneTip,
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

/// Water keeps the dry block's surface properties and static geometry. Lava
/// blocks have no geometry; their travel rules belong to the movement owner.
/// Other uncertain contexts (climbables, webs, dynamic shapes) cannot acquire
/// support merely because their representative shape is empty.
pub fn motion_info(id: BlockStateId) -> MotionInfo {
    let state = state_info(id);
    // Context-dependent movement is resolved by the shared player collision
    // path. IDs and rules are verified against the official 26.3 client.
    let surface = match id {
        2362 => Some(Surface::Cobweb),
        30519 => Some(Surface::PowderSnow),
        24063..=24094 => {
            // The bottom plate collides only for bottom=true, distance>0.
            Some(Surface::Scaffolding { bottom: matches!(id, 24065..=24078) })
        }
        18651..=18652 => Some(Surface::BubbleColumn { down: id == 18651 }),
        33566..=33605 => Some(if matches!(id, 33570..=33571) {
            Surface::DripstoneTip
        } else {
            Surface::Ordinary
        }),
        // No block entities are published: shulkers remain closed, moving
        // piston placeholders have the client's empty fallback collision.
        3801..=3812 | 16845..=16946 | 33713..=33744 => Some(Surface::Ordinary),
        _ => None,
    };
    if let Some(surface) = surface {
        return MotionInfo { surface, ..MotionInfo::default() };
    }
    // Sweet berry bushes have empty collision, but apply an inside-block
    // movement multiplier at every age (official Java 26.3 state IDs).
    if matches!(id, 24298..=24301) {
        return MotionInfo { surface: Surface::BerryBush, ..MotionInfo::default() };
    }
    if xz_offset(id) || matches!(climb(id), Climb::Ladder(_) | Climb::Vine) {
        return MotionInfo::default();
    }
    if state.contextual_geometry() {
        return MotionInfo { surface: Surface::Unsupported, ..MotionInfo::default() };
    }
    if state.flags.has_fluid() {
        let fluid = fluid_info(id);
        return match fluid.kind {
            FluidKind::Water if fluid.supported => motion_info(fluid.dry_state),
            FluidKind::Lava if fluid.dry_state == 0 => MotionInfo::default(),
            _ => MotionInfo { surface: Surface::Unsupported, ..MotionInfo::default() },
        };
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

/// Climbing context in horizontal facing order N, E, S, W. Of the climbable
/// tag, ladders, vines and scaffolding are supported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Climb {
    #[default]
    None,
    Ladder(u8),
    /// Climbable only directly above a ladder with the same facing.
    OpenTrapdoor(u8),
    /// Vines, cave vines, weeping vines and twisting vines: no collision and
    /// no facing; climbed by jumping or pushing against a wall.
    Vine,
    Scaffolding,
}

/// The vine family of the `#climbable` tag.
const VINES: [&str; 7] = [
    "minecraft:vine",
    "minecraft:cave_vines",
    "minecraft:cave_vines_plant",
    "minecraft:weeping_vines",
    "minecraft:weeping_vines_plant",
    "minecraft:twisting_vines",
    "minecraft:twisting_vines_plant",
];
const VINE: u8 = 9;

static CLIMB: OnceLock<Box<[u8]>> = OnceLock::new();

/// One byte per state, derived once from the registry keys and shared.
pub fn climb(id: BlockStateId) -> Climb {
    let table = CLIMB.get_or_init(|| {
        let mut table = vec![0u8; data::STATES.len()].into_boxed_slice();
        for &(key, id) in crate::blockstates::BLOCK_STATES {
            let (name, props) = key.split_once('[').unwrap_or((key, ""));
            if name == "minecraft:scaffolding" {
                table[id as usize] = 10;
                continue;
            }
            if VINES.contains(&name) {
                table[id as usize] = VINE;
                continue;
            }
            let ladder = name == "minecraft:ladder";
            if !ladder && !(name.ends_with("_trapdoor") && props.contains("open=true")) {
                continue;
            }
            let Some(facing) = ["north", "east", "south", "west"]
                .iter()
                .position(|f| props.contains(&format!("facing={f}")))
            else {
                continue;
            };
            table[id as usize] = 1 + facing as u8 + if ladder { 0 } else { 4 };
        }
        table
    });
    match table.get(id as usize).copied().unwrap_or(0) {
        0 => Climb::None,
        n @ 1..=4 => Climb::Ladder(n - 1),
        VINE => Climb::Vine,
        10 => Climb::Scaffolding,
        n => Climb::OpenTrapdoor(n - 5),
    }
}

/// Bamboo stalks and speleothems use static shapes translated per column.
pub fn xz_offset(id: BlockStateId) -> bool {
    matches!(id, 18636..=18647 | 33566..=33605)
}

/// Geometry still unsupported after the player context and the server's
/// no-block-entity fallbacks have been accounted for. The raw catalog flags
/// remain available to non-player simulations with different shape rules.
pub fn contextual_geometry(id: BlockStateId) -> bool {
    state_info(id).contextual_geometry()
        && !xz_offset(id)
        && !matches!(id, 3801..=3812 | 16845..=16946 | 24063..=24094 | 30519)
}

/// Translation from the catalog shape to the shape at block column (x, z).
/// The catalog captured offset shapes at the origin, where vanilla's hash
/// yields (-0.25, 0, -0.25) for bamboo and (-0.125, 0, -0.125) for
/// speleothems, whose target horizontal offset is clamped to 0.125.
pub fn collision_offset(id: BlockStateId, x: i32, z: i32) -> [f64; 3] {
    if !xz_offset(id) {
        return [0.0; 3];
    }
    // Mth.getSeed(x, 0, z) with Java int/long overflow; the 0.25 clamp of
    // BlockBehaviour.getMaxHorizontalOffset cannot bind for 4-bit inputs.
    let seed = i64::from(x.wrapping_mul(3_129_871)) ^ i64::from(z).wrapping_mul(116_129_781);
    let seed = seed.wrapping_mul(seed).wrapping_mul(42_317_861).wrapping_add(seed.wrapping_mul(11));
    let seed = seed >> 16;
    let max = if matches!(id, 33566..=33605) { 0.125 } else { 0.25 };
    let axis =
        |bits: i64| ((f64::from((bits & 15) as f32 / 15.0) - 0.5) * 0.5).clamp(-max, max) + max;
    [axis(seed), 0.0, axis(seed >> 8)]
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
    size_of_val(data::STATES)
        + size_of_val(data::SHAPES)
        + size_of_val(data::BOXES)
        + size_of_val(fluid_data::FLUIDS)
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
#[path = "tests/tests_terrain.rs"]
mod tests;
