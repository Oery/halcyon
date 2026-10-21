pub mod blockstates;
pub mod disk_nbt;
pub mod loader;
pub mod terrain;

use thiserror::Error;

pub use loader::{LoadedWorld, load_dir};
pub use terrain::{
    Aabb, BlockKind, BlockProperties, BlockStateId, COLLISION_MAX_OFFSET, COLLISION_MIN_OFFSET,
    MAX_BLOCK_Y, MIN_BLOCK_Y, MotionInfo, SECTION_COUNT, Section, StateInfo, Surface, TerrainFlags,
    block_properties, collision_catalog_bytes, motion_info, state_info, try_state_info,
};

use std::sync::Arc;

/// Authoritative block queries shared by imported terrain and arena overlays.
pub trait BlockView {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId>;
    fn contains_column(&self, x: i32, z: i32) -> bool;
}

impl BlockView for World {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        World::block_at(self, x, y, z)
    }
    fn contains_column(&self, x: i32, z: i32) -> bool {
        World::contains_column(self, x, z)
    }
}

#[derive(Error, Debug)]
pub enum LoadError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("bad nbt in chunk {chunk}: {reason}")]
    BadNbt { chunk: String, reason: String },

    #[error("unsupported compression {ctype} in chunk {chunk}")]
    UnsupportedCompression { chunk: String, ctype: u8 },

    #[error("no chunks loaded from {0}")]
    NoChunks(String),

    #[error("unknown block state {state} in chunk {chunk}")]
    UnknownBlockState { chunk: String, state: String },
}

/// Immutable hot-path world: a flat grid over the loaded rect, O(1) lookup
/// with no hashing. Each chunk's section bytes are pre-encoded once and
/// shared across connections via `Arc`; the only per-send copies are the
/// 296-byte heightmap array and empty light masks.
pub struct World {
    origin_x: i32,
    origin_z: i32,
    width: u32,
    depth: u32,
    chunks: Box<[Option<Arc<Chunk>>]>,
    pub spawn: (f64, f64, f64, f32, f32),
    pub wire_bytes: usize,
    /// Section index and direct-state payload, excluding existing wire data,
    /// Arc headers, allocator overhead, and the shared collision catalog.
    pub terrain_bytes: usize,
}

/// One pre-encoded chunk column. `data` holds exactly the `ChunkData.data`
/// section bytes; `heights` is the 37-long `MOTION_BLOCKING` array.
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub data: Arc<[u8]>,
    pub heights: Arc<[i64; 37]>,
    pub sections: Box<[Section; SECTION_COUNT]>,
}

impl World {
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Chunk>> {
        self.chunks.iter().filter_map(Option::as_ref)
    }
    pub fn from_loaded(loaded: LoadedWorld) -> World {
        let min_x = loaded.chunks.iter().map(|c| c.x).min().unwrap_or(0);
        let max_x = loaded.chunks.iter().map(|c| c.x).max().unwrap_or(0);
        let min_z = loaded.chunks.iter().map(|c| c.z).min().unwrap_or(0);
        let max_z = loaded.chunks.iter().map(|c| c.z).max().unwrap_or(0);
        let width = (max_x - min_x + 1) as u32;
        let depth = (max_z - min_z + 1) as u32;
        let mut chunks: Box<[Option<Arc<Chunk>>]> = (0..width * depth).map(|_| None).collect();
        let mut wire_bytes = 0usize;
        let mut terrain_bytes = 0usize;
        for c in loaded.chunks {
            wire_bytes += c.wire_bytes;
            terrain_bytes += std::mem::size_of::<[Section; SECTION_COUNT]>()
                + c.sections.iter().map(Section::backing_bytes).sum::<usize>();
            let idx = ((c.z - min_z) as u32 * width + (c.x - min_x) as u32) as usize;
            chunks[idx] = Some(Arc::new(Chunk {
                x: c.x,
                z: c.z,
                data: Arc::from(c.data.into_boxed_slice()),
                heights: Arc::new(c.heights),
                sections: c.sections,
            }));
        }
        World {
            origin_x: min_x,
            origin_z: min_z,
            width,
            depth,
            chunks,
            spawn: loaded.spawn,
            wire_bytes,
            terrain_bytes,
        }
    }

    pub fn get(&self, x: i32, z: i32) -> Option<&Arc<Chunk>> {
        let dx = x.checked_sub(self.origin_x)? as u32;
        let dz = z.checked_sub(self.origin_z)? as u32;
        if dx >= self.width || dz >= self.depth {
            return None;
        }
        self.chunks[(dz * self.width + dx) as usize].as_ref()
    }

    pub fn contains_column(&self, chunk_x: i32, chunk_z: i32) -> bool {
        self.get(chunk_x, chunk_z).is_some()
    }

    /// A missing column is distinct from loaded air. Outside-height queries
    /// also return None; use the height constants and `contains_column` when
    /// distinguishing open vertical space from an unloaded horizontal edge.
    #[inline]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        if !(MIN_BLOCK_Y..MAX_BLOCK_Y).contains(&y) {
            return None;
        }
        let chunk = self.get(x.div_euclid(16), z.div_euclid(16))?;
        let relative_y = (y - MIN_BLOCK_Y) as usize;
        Some(chunk.sections[relative_y >> 4].block_at(
            x.rem_euclid(16) as usize,
            relative_y & 15,
            z.rem_euclid(16) as usize,
        ))
    }

    /// All loaded chunks sorted by distance from the center column.
    pub fn chunks_by_distance(&self, cx: i32, cz: i32) -> Vec<&Arc<Chunk>> {
        let mut out: Vec<&Arc<Chunk>> = self.chunks.iter().filter_map(|c| c.as_ref()).collect();
        out.sort_by_key(|c| {
            let dx = c.x - cx;
            let dz = c.z - cz;
            dx * dx + dz * dz
        });
        out
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }

    pub fn load(path: &std::path::Path) -> Result<World, LoadError> {
        let start = std::time::Instant::now();
        let loaded = load_dir(path)?;
        eprintln!(
            "world: {} chunks, {} wire bytes, missing={} remapped_biomes={} in {}ms",
            loaded.chunks.len(),
            loaded.chunks.iter().map(|c| c.wire_bytes).sum::<usize>(),
            loaded.missing_blocks,
            loaded.remapped_biomes,
            start.elapsed().as_millis(),
        );
        let world = World::from_loaded(loaded);
        eprintln!(
            "world: {} terrain query bytes, {} shared collision catalog bytes",
            world.terrain_bytes,
            collision_catalog_bytes()
        );
        Ok(world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "loads a local map and reports hardware-specific component measurements"]
    fn terrain_world_measurement() {
        let path =
            std::env::var_os("HALCYON_TERRAIN_WORLD").unwrap_or_else(|| "worlds/lobby".into());
        let world = World::load(std::path::Path::new(&path)).unwrap();
        let dense = world
            .iter()
            .flat_map(|chunk| chunk.sections.iter())
            .filter(|section| matches!(section, Section::Dense(_)))
            .count();
        let chunks: Vec<_> = world.iter().collect();
        let start = std::time::Instant::now();
        let queries = 1_000_000;
        for i in 0..queries {
            let chunk = chunks[i % chunks.len()];
            let x = chunk.x * 16 + (i & 15) as i32;
            let z = chunk.z * 16 + ((i >> 4) & 15) as i32;
            let y = 48 + ((i >> 8) & 31) as i32;
            let state = std::hint::black_box(world.block_at(x, y, z).unwrap());
            std::hint::black_box(state_info(state).collision_boxes());
        }
        eprintln!(
            "terrain: chunks={} dense_sections={dense} query_bytes={} catalog_bytes={} ns_per_query={:.1}",
            world.chunk_count(),
            world.terrain_bytes,
            collision_catalog_bytes(),
            start.elapsed().as_nanos() as f64 / queries as f64
        );
    }

    #[test]
    fn block_queries_handle_negative_coordinates_and_missing_columns() {
        let mut sections = Box::new(std::array::from_fn(|_| Section::default()));
        let mut states = Box::new([0; 4096]);
        states[(15 << 8) | (15 << 4) | 15] = 1;
        sections[0] = Section::Dense(states);
        sections[SECTION_COUNT - 1] = Section::Uniform(2);
        let world = World::from_loaded(LoadedWorld {
            chunks: vec![loader::LoadedChunk {
                x: -1,
                z: -1,
                data: vec![],
                heights: [0; 37],
                wire_bytes: 0,
                sections,
            }],
            spawn: (0.0, 0.0, 0.0, 0.0, 0.0),
            missing_blocks: 0,
            remapped_biomes: 0,
        });
        assert_eq!(world.block_at(-1, MIN_BLOCK_Y + 15, -1), Some(1));
        assert_eq!(world.block_at(-16, MIN_BLOCK_Y, -16), Some(0));
        assert_eq!(world.block_at(-1, MAX_BLOCK_Y - 1, -1), Some(2));
        assert_eq!(world.block_at(-1, MIN_BLOCK_Y - 1, -1), None);
        assert_eq!(world.block_at(-1, MAX_BLOCK_Y, -1), None);
        assert_eq!(world.block_at(0, 0, -1), None);
        assert_eq!(world.block_at(i32::MIN, 0, i32::MAX), None);
        assert!(world.contains_column(-1, -1));
        assert!(!world.contains_column(0, -1));
        assert_eq!(world.terrain_bytes, std::mem::size_of::<[Section; SECTION_COUNT]>() + 8192);
    }
}
