//! Shared immutable placement and mining metadata for the supported arena catalog.
use std::sync::OnceLock;

use crate::blockstates::{BLOCK_STATES, block_state_id};
use crate::{BlockStateId, BlockView, FluidKind, block_properties, fluid_info};

mod data;
pub mod entities;
mod neighbors;
pub use neighbors::{MAX_EDIT_CELLS, neighbor_state, updates};

fn connection_exception(state: BlockStateId) -> bool {
    data::STATES[state as usize].2 & 32 != 0
}
fn wall_cover(state: BlockStateId, index: usize) -> bool {
    data::STATES[state as usize].2 & (1 << index) != 0
}
fn solid(state: BlockStateId) -> bool {
    data::STATES[state as usize].2 & 64 != 0
}
fn center_down(state: BlockStateId) -> bool {
    data::STATES[state as usize].2 & 128 != 0
}

pub struct ToolSpeed {
    pub item: u16,
    pub speed: f32,
    pub correct: bool,
}

pub struct BlockData {
    pub name: &'static str,
    pub default: BlockStateId,
    pub hardness: f32,
    pub requires_tool: bool,
    /// A target outside the supported catalog may consume use instead of placing.
    pub use_handler: bool,
    pub place_sound: &'static str,
    pub break_sound: &'static str,
    pub open_sound: &'static str,
    pub close_sound: &'static str,
    pub tools: &'static [ToolSpeed],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Cube,
    Axis,
    Slab,
    Stairs,
    Fence,
    Wall,
    Gate,
    Door,
    Trapdoor,
    Sign,
    Jukebox,
}

pub const DIRECTIONS: [[i32; 3]; 6] =
    [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
pub const HORIZONTAL: [&str; 4] = ["north", "east", "south", "west"];

pub fn metadata(state: BlockStateId) -> &'static BlockData {
    &data::BLOCKS[data::STATES[state as usize].0 as usize]
}

pub fn named(name: &str) -> Option<&'static BlockData> {
    static BY_NAME: OnceLock<std::collections::HashMap<&'static str, &'static BlockData>> =
        OnceLock::new();
    BY_NAME.get_or_init(|| data::BLOCKS.iter().map(|b| (b.name, b)).collect()).get(name).copied()
}

pub fn key(state: BlockStateId) -> &'static str {
    static KEYS: OnceLock<Box<[&'static str]>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys = vec![""; BLOCK_STATES.len()];
        for &(key, id) in BLOCK_STATES {
            keys[id as usize] = key;
        }
        keys.into_boxed_slice()
    })[state as usize]
}

pub fn property(state: BlockStateId, name: &str) -> Option<&'static str> {
    key(state).split_once('[')?.1.strip_suffix(']')?.split(',').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Change existing properties only; canonical registry keys remain ordered.
pub fn with(state: BlockStateId, values: &[(&str, &str)]) -> Option<BlockStateId> {
    let Some((name, props)) = key(state).split_once('[') else {
        return Some(state);
    };
    let mut result = String::with_capacity(128);
    result.push_str(name);
    result.push('[');
    for (index, prop) in props.strip_suffix(']')?.split(',').enumerate() {
        if index != 0 {
            result.push(',');
        }
        let (key, value) = prop.split_once('=')?;
        result.push_str(key);
        result.push('=');
        result.push_str(values.iter().find(|(k, _)| *k == key).map_or(value, |(_, v)| *v));
    }
    result.push(']');
    block_state_id(&result).map(|id| id as BlockStateId)
}

pub fn sturdy(state: BlockStateId, face: usize) -> bool {
    data::STATES[state as usize].1 & (1 << face) != 0
}

pub fn placement(state: BlockStateId) -> Option<Placement> {
    let name = metadata(state).name.strip_prefix("minecraft:")?;
    Some(if name.ends_with("_slab") {
        Placement::Slab
    } else if name.ends_with("_stairs") {
        Placement::Stairs
    } else if name.ends_with("_fence_gate") {
        Placement::Gate
    } else if name.ends_with("_fence") {
        Placement::Fence
    } else if name.ends_with("_wall") {
        Placement::Wall
    } else if name.ends_with("_trapdoor") && name != "iron_trapdoor" {
        Placement::Trapdoor
    } else if name.ends_with("_door") && name != "iron_door" {
        Placement::Door
    } else if name.ends_with("_sign") {
        Placement::Sign
    } else if name == "jukebox" {
        Placement::Jukebox
    } else if name.ends_with("_log")
        || name.ends_with("_wood")
        || (name.ends_with("_stem") && property(state, "axis").is_some())
        || name.ends_with("_hyphae")
        || matches!(name, "bamboo_block" | "stripped_bamboo_block" | "deepslate")
    {
        Placement::Axis
    } else if matches!(
        name,
        "stone"
            | "granite"
            | "polished_granite"
            | "diorite"
            | "polished_diorite"
            | "andesite"
            | "polished_andesite"
            | "cobblestone"
            | "mossy_cobblestone"
            | "deepslate"
            | "cobbled_deepslate"
            | "polished_deepslate"
            | "deepslate_bricks"
            | "cracked_deepslate_bricks"
            | "deepslate_tiles"
            | "cracked_deepslate_tiles"
            | "chiseled_deepslate"
            | "bricks"
            | "stone_bricks"
            | "mossy_stone_bricks"
            | "cracked_stone_bricks"
            | "chiseled_stone_bricks"
            | "sandstone"
            | "cut_sandstone"
            | "smooth_sandstone"
            | "chiseled_sandstone"
            | "red_sandstone"
            | "cut_red_sandstone"
            | "smooth_red_sandstone"
            | "chiseled_red_sandstone"
            | "glass"
            | "obsidian"
            | "dirt"
            | "coarse_dirt"
            | "sand"
            | "red_sand"
            | "gravel"
            | "terracotta"
    ) || name.ends_with("_planks")
        || name.ends_with("_wool")
        || name.ends_with("_concrete")
        || (name.ends_with("_terracotta") && !name.ends_with("_glazed_terracotta"))
        || name.ends_with("_stained_glass")
    {
        Placement::Cube
    } else {
        return None;
    })
}

pub fn offset(pos: [i32; 3], direction: usize) -> [i32; 3] {
    std::array::from_fn(|i| pos[i] + DIRECTIONS[direction][i])
}

/// Unsupported use handlers must not accidentally place the held item.
pub fn reserves_use(state: BlockStateId) -> bool {
    match placement(state) {
        Some(Placement::Sign) => true,
        None => metadata(state).use_handler,
        _ => false,
    }
}

pub fn horizontal(name: &str) -> usize {
    match name {
        "north" => 2,
        "east" => 5,
        "south" => 3,
        _ => 4,
    }
}

pub fn facing(yaw: f32) -> &'static str {
    let turns = ((f64::from(yaw) / 90.0 + 0.5).floor() as i32).rem_euclid(4);
    HORIZONTAL[((turns + 2) % 4) as usize]
}

pub fn replacement(state: BlockStateId) -> BlockStateId {
    if fluid_info(state).kind == FluidKind::Water {
        named("minecraft:water").expect("water registry").default
    } else {
        0
    }
}

pub fn replaceable(state: BlockStateId) -> bool {
    block_properties(state).replaceable()
}

/// A bounded candidate overlay shared by support checks and falling-route validation.
pub struct PlannedView<'a, V> {
    pub base: &'a V,
    pub cells: &'a [([i32; 3], BlockStateId)],
}

#[cfg(test)]
#[path = "../tests/tests_blocks.rs"]
mod tests;

impl<V: BlockView> BlockView for PlannedView<'_, V> {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        self.cells
            .iter()
            .rev()
            .find(|(p, _)| *p == [x, y, z])
            .map(|(_, s)| *s)
            .or_else(|| self.base.block_at(x, y, z))
    }
    fn contains_column(&self, x: i32, z: i32) -> bool {
        self.base.contains_column(x, z)
    }
}
