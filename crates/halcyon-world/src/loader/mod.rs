//! Cold-path Anvil region loader.
//!
//! Reads `<dir>/region/r.<rx>.<rz>.mca` files once at startup and
//! pre-encodes every chunk column into the exact wire bytes `ChunkData`
//! sends. All allocation, NBT parsing, and palette mapping happens here;
//! the hot path borrows immutable wire bytes or queries retained state arrays.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;

use crate::disk_nbt::DiskNbt;
use crate::{LoadError, MIN_BLOCK_Y, SECTION_COUNT, Section};

mod section;

use section::{encode_air_section, encode_section};

/// Overworld shape from the synced `dimension_type` entry: 24 sections,
/// `Y` in `-4..20`.
const MIN_Y_SECTION: i8 = (MIN_BLOCK_Y / 16) as i8;
/// Packed heightmap longs per chunk column: 256 columns, 9 bits each.
const HEIGHTS_LEN: usize = 37;
/// Spawn fallback when `level.dat` is missing: today's hardcoded position.
const FALLBACK_SPAWN: (f64, f64, f64, f32, f32) = (8.5, 64.0, 8.5, 0.0, 0.0);

pub struct LoadedChunk {
    pub block_entities: Vec<crate::blocks::entities::BlockEntity>,
    pub x: i32,
    pub z: i32,
    pub data: Vec<u8>,
    pub heights: [i64; HEIGHTS_LEN],
    pub wire_bytes: usize,
    pub sections: Box<[Section; SECTION_COUNT]>,
}

pub struct LoadedWorld {
    pub chunks: Vec<LoadedChunk>,
    pub spawn: (f64, f64, f64, f32, f32),
    pub missing_blocks: usize,
    pub remapped_biomes: usize,
}

pub fn load_dir(path: &Path) -> Result<LoadedWorld, LoadError> {
    let region_dir = path.join("region");
    let entries = std::fs::read_dir(&region_dir).map_err(LoadError::Io)?;
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(LoadError::Io)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("r.") && name.ends_with(".mca") {
            files.push(entry.path());
        }
    }
    files.sort();

    let mut chunks: Vec<LoadedChunk> = Vec::new();
    let mut missing_blocks = 0usize;
    let mut remapped_biomes = 0usize;
    for file in &files {
        let (miss, remap) = load_region(file, &mut chunks)?;
        missing_blocks += miss;
        remapped_biomes += remap;
    }
    if chunks.is_empty() {
        return Err(LoadError::NoChunks(region_dir.display().to_string()));
    }

    let spawn = load_spawn(&path.join("level.dat"));
    Ok(LoadedWorld { chunks, spawn, missing_blocks, remapped_biomes })
}

fn load_region(path: &Path, out: &mut Vec<LoadedChunk>) -> Result<(usize, usize), LoadError> {
    let label = path.display().to_string();
    let start = std::time::Instant::now();
    let before = out.len();
    let mut raw = Vec::new();
    File::open(path).and_then(|mut f| f.read_to_end(&mut raw)).map_err(LoadError::Io)?;
    if raw.len() < 8192 {
        return Err(LoadError::BadNbt {
            chunk: label.clone(),
            reason: format!("region file shorter than 8 KiB header: {}", raw.len()),
        });
    }

    let mut missing_blocks = 0usize;
    let mut remapped_biomes = 0usize;
    for i in 0..1024 {
        let loc = u32::from_be_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap());
        let sectors = (loc >> 8) as usize;
        let count = (loc & 0xFF) as usize;
        if sectors == 0 || count == 0 {
            continue;
        }
        let start = sectors * 4096;
        if start + 5 > raw.len() {
            eprintln!("world: {label} entry {i} points outside file, skipping");
            continue;
        }
        let len = i32::from_be_bytes(raw[start..start + 4].try_into().unwrap());
        let ctype = raw[start + 4];
        if len < 1 || start + 4 + len as usize > raw.len() {
            eprintln!("world: {label} entry {i} has bad length {len}, skipping");
            continue;
        }
        let payload = &raw[start + 5..start + 4 + len as usize];
        let name = format!("{label}#{i}");
        match decode_chunk_payload(&name, ctype, payload) {
            Ok(Some(chunk)) => {
                missing_blocks += chunk.missing;
                remapped_biomes += chunk.remapped;
                out.push(chunk.chunk);
            }
            Ok(None) => {}
            Err(
                e
                @ (LoadError::UnknownBlockState { .. } | LoadError::UnsupportedBlockEntity { .. }),
            ) => return Err(e),
            Err(e) => {
                eprintln!("world: skipping chunk {name}: {e}");
            }
        }
    }
    if out.is_empty() {
        eprintln!("world: {label}: no chunks loaded");
    } else {
        eprintln!(
            "world: {label}: {} chunks loaded in {}ms",
            out.len() - before,
            start.elapsed().as_millis(),
        );
    }
    Ok((missing_blocks, remapped_biomes))
}

struct Decoded {
    chunk: LoadedChunk,
    missing: usize,
    remapped: usize,
}

fn decode_chunk_payload(
    name: &str,
    ctype: u8,
    payload: &[u8],
) -> Result<Option<Decoded>, LoadError> {
    if ctype != 2 {
        return Err(LoadError::UnsupportedCompression { chunk: name.to_owned(), ctype });
    }
    let mut decompressed = Vec::new();
    ZlibDecoder::new(payload).read_to_end(&mut decompressed).map_err(|e| LoadError::BadNbt {
        chunk: name.to_owned(),
        reason: format!("zlib error: {e}"),
    })?;
    let mut slice = decompressed.as_slice();
    let root = DiskNbt::read_root(&mut slice).map_err(|e| match e {
        LoadError::BadNbt { reason, .. } => LoadError::BadNbt { chunk: name.to_owned(), reason },
        other => other,
    })?;

    let x = root.get("xPos").and_then(|v| v.as_i32()).ok_or_else(|| LoadError::BadNbt {
        chunk: name.to_owned(),
        reason: "missing xPos".to_owned(),
    })?;
    let z = root.get("zPos").and_then(|v| v.as_i32()).ok_or_else(|| LoadError::BadNbt {
        chunk: name.to_owned(),
        reason: "missing zPos".to_owned(),
    })?;

    let missing = 0usize;
    let mut remapped = 0usize;
    let mut data = Vec::with_capacity(SECTION_COUNT * 512);
    let mut terrain_sections = Box::new(std::array::from_fn(|_| Section::default()));
    let heights = load_heights(name, &root)?;

    // Index Anvil sections by Y so absent ones encode as air.
    let mut by_y: BTreeMap<i8, &DiskNbt> = BTreeMap::new();
    if let Some(sections) = root.get("sections").and_then(|v| v.as_list()) {
        for section in sections {
            if let Some(y) = section.get("Y").and_then(|v| v.as_i8()) {
                by_y.insert(y, section);
            }
        }
    }
    for (idx, terrain_section) in terrain_sections.iter_mut().enumerate() {
        let y = MIN_Y_SECTION + idx as i8;
        match by_y.get(&y) {
            Some(section) => {
                *terrain_section = encode_section(name, section, &mut data, &mut remapped)?;
            }
            None => encode_air_section(&mut data),
        }
    }

    let wire_bytes = data.len();
    let mut block_entities = Vec::new();
    if let Some(entries) = root.get("block_entities").and_then(DiskNbt::as_list) {
        use crate::blocks::entities::{
            BlockEntity, BlockEntityData, MAX_BLOCK_ENTITIES_PER_COLUMN, Sign,
        };
        for entry in entries {
            let Some(pos) = (|| {
                Some([
                    entry.get("x")?.as_i32()?,
                    entry.get("y")?.as_i32()?,
                    entry.get("z")?.as_i32()?,
                ])
            })() else {
                continue;
            };
            if pos[0].div_euclid(16) != x
                || pos[2].div_euclid(16) != z
                || !(crate::MIN_BLOCK_Y..crate::MAX_BLOCK_Y).contains(&pos[1])
            {
                continue;
            }
            let index = (pos[1] - crate::MIN_BLOCK_Y) as usize;
            let state = terrain_sections[index / 16].block_at(
                pos[0].rem_euclid(16) as usize,
                index % 16,
                pos[2].rem_euclid(16) as usize,
            );
            let block = crate::blocks::metadata(state).name;
            let data = if block.ends_with("_sign") {
                let Some(sign) = Sign::from_nbt(entry) else {
                    return Err(LoadError::UnsupportedBlockEntity {
                        chunk: name.to_owned(),
                        reason: "invalid or excessive sign text".to_owned(),
                    });
                };
                BlockEntityData::Sign(std::sync::Arc::new(sign))
            } else if block == "minecraft:jukebox" {
                let disc = entry
                    .get("RecordItem")
                    .and_then(|v| v.get("id"))
                    .and_then(DiskNbt::as_str)
                    .filter(|s| s.starts_with("minecraft:music_disc_"))
                    .map(str::to_owned);
                BlockEntityData::Jukebox { disc }
            } else {
                continue;
            };
            if block_entities.len() == MAX_BLOCK_ENTITIES_PER_COLUMN
                || block_entities.iter().any(|entry: &BlockEntity| entry.pos == pos)
            {
                return Err(LoadError::UnsupportedBlockEntity {
                    chunk: name.to_owned(),
                    reason: "excessive or duplicate supported block entities".to_owned(),
                });
            }
            block_entities.push(BlockEntity { pos, data });
        }
    }
    Ok(Some(Decoded {
        chunk: LoadedChunk {
            block_entities,
            x,
            z,
            data,
            heights,
            wire_bytes,
            sections: terrain_sections,
        },
        missing,
        remapped,
    }))
}

fn load_heights(name: &str, root: &DiskNbt) -> Result<[i64; HEIGHTS_LEN], LoadError> {
    let mut heights = [0i64; HEIGHTS_LEN];
    let Some(mb) = root
        .get("Heightmaps")
        .and_then(|h| h.get("MOTION_BLOCKING"))
        .and_then(|v| v.as_long_array())
    else {
        return Ok(heights);
    };
    if mb.len() != HEIGHTS_LEN {
        eprintln!(
            "world: {name}: MOTION_BLOCKING has {} longs, want {HEIGHTS_LEN}; zero-filling",
            mb.len()
        );
        return Ok(heights);
    }
    heights.copy_from_slice(mb);
    Ok(heights)
}

fn load_spawn(path: &Path) -> (f64, f64, f64, f32, f32) {
    let Ok(raw) = std::fs::read(path) else {
        eprintln!("world: no level.dat, using fallback spawn");
        return FALLBACK_SPAWN;
    };
    // level.dat is gzip-compressed NBT.
    let mut decompressed = Vec::new();
    if flate2::read::GzDecoder::new(raw.as_slice()).read_to_end(&mut decompressed).is_err() {
        eprintln!("world: level.dat gunzip failed, using fallback spawn");
        return FALLBACK_SPAWN;
    }
    let mut slice = decompressed.as_slice();
    let Ok(root) = DiskNbt::read_root(&mut slice) else {
        eprintln!("world: level.dat NBT parse failed, using fallback spawn");
        return FALLBACK_SPAWN;
    };
    let Some(data) = root.get("Data") else {
        eprintln!("world: level.dat has no Data, using fallback spawn");
        return FALLBACK_SPAWN;
    };
    // Current form: `spawn{pos: [x,y,z], yaw, pitch}`. Legacy form:
    // `SpawnX/SpawnY/SpawnZ` ints.
    if let Some(spawn) = data.get("spawn")
        && let Some(pos) = spawn.get("pos").and_then(|v| v.as_int_array())
        && pos.len() == 3
    {
        let yaw = spawn.get("yaw").and_then(|v| v.as_float()).unwrap_or(0.0);
        let pitch = spawn.get("pitch").and_then(|v| v.as_float()).unwrap_or(0.0);
        return (pos[0] as f64 + 0.5, pos[1] as f64, pos[2] as f64 + 0.5, yaw, pitch);
    }
    if data.get("spawn").is_some() {
        eprintln!("world: level.dat spawn malformed, using fallback spawn");
        return FALLBACK_SPAWN;
    }
    let x = data.get("SpawnX").and_then(|v| v.as_i32()).unwrap_or(8);
    let y = data.get("SpawnY").and_then(|v| v.as_i32()).unwrap_or(64);
    let z = data.get("SpawnZ").and_then(|v| v.as_i32()).unwrap_or(8);
    (x as f64 + 0.5, y as f64, z as f64 + 0.5, 0.0, 0.0)
}
