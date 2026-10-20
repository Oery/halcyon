//! Cold-path Anvil region loader.
//!
//! Reads `<dir>/region/r.<rx>.<rz>.mca` files once at startup and
//! pre-encodes every chunk column into the exact wire bytes `ChunkData`
//! sends. All allocation, NBT parsing, and palette mapping happens here;
//! the hot path only borrows the resulting byte slices.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;

use crate::LoadError;
use crate::blockstates::{block_state_id, canonical_key};
use crate::disk_nbt::DiskNbt;

/// Overworld shape from the synced `dimension_type` entry: 24 sections,
/// `Y` in `-4..20`.
const SECTION_COUNT: usize = 24;
const MIN_Y_SECTION: i8 = -4;
/// Packed heightmap longs per chunk column: 256 columns, 9 bits each.
const HEIGHTS_LEN: usize = 37;
/// Spawn fallback when `level.dat` is missing: today's hardcoded position.
const FALLBACK_SPAWN: (f64, f64, f64, f32, f32) = (8.5, 64.0, 8.5, 0.0, 0.0);

pub struct LoadedChunk {
    pub x: i32,
    pub z: i32,
    pub data: Vec<u8>,
    pub heights: [i64; HEIGHTS_LEN],
    pub wire_bytes: usize,
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
            Err(e) => {
                eprintln!("world: skipping chunk {name}: {e}");
            }
        }
    }
    if out.is_empty() {
        eprintln!("world: {label}: no chunks loaded");
    } else {
        eprintln!(
            "world: {label}: {} chunks, {missing_blocks} missing blocks remapped to air in {}ms",
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

    let mut missing = 0usize;
    let mut remapped = 0usize;
    let mut data = Vec::with_capacity(SECTION_COUNT * 512);
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
    for idx in 0..SECTION_COUNT {
        let y = MIN_Y_SECTION + idx as i8;
        match by_y.get(&y) {
            Some(section) => {
                encode_section(section, &mut data, &mut missing, &mut remapped);
            }
            None => encode_air_section(&mut data),
        }
    }

    let wire_bytes = data.len();
    Ok(Some(Decoded { chunk: LoadedChunk { x, z, data, heights, wire_bytes }, missing, remapped }))
}

/// Encode one Anvil section into wire `Chunk Section` bytes and append.
fn encode_section(section: &DiskNbt, out: &mut Vec<u8>, missing: &mut usize, remapped: &mut usize) {
    let Some(block_states) = section.get("block_states") else {
        encode_air_section(out);
        return;
    };
    let Some(palette) = block_states.get("palette").and_then(|v| v.as_list()) else {
        encode_air_section(out);
        return;
    };

    // Map the Anvil string palette to global IDs. 26.x entries are
    // `{id, properties}` (lowercase); older maps use `{Name, Properties}`.
    // Values are the same strings either way, so one table covers both.
    let mut ids: Vec<u32> = Vec::with_capacity(palette.len());
    for entry in palette {
        let name = entry
            .get("id")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("Name").and_then(|v| v.as_str()))
            .unwrap_or("minecraft:air");
        let props_tag = entry.get("properties").or_else(|| entry.get("Properties"));
        let mut props: Vec<(String, String)> = Vec::new();
        if let Some(p) = props_tag.and_then(|v| v.as_compound()) {
            for (k, v) in p {
                if let DiskNbt::Str(s) = v {
                    props.push((k.clone(), s.clone()));
                }
            }
        }
        let key = canonical_key(name, &props);
        match block_state_id(&key) {
            Some(id) => ids.push(id),
            None => {
                *missing += 1;
                ids.push(0);
            }
        }
    }

    // Biome palette ignored: every section encodes single plains (index 0)
    // because the synced `worldgen/biome` registry only contains plains.
    *remapped += 1;

    if let Some(data) = block_states.get("data").and_then(|v| v.as_long_array()) {
        encode_indirect_section(&ids, data, out);
    } else {
        encode_single_valued(&ids, out);
    }
    // Biome container: always single plains (BPE 0 + id 0 = 2 bytes).
    out.extend_from_slice(&[0x00, 0x00]);
}

/// Indirect container: re-emit Anvil indices at wire BPE
/// (`max(4, ceil(log2(palette_len)))`), palette as global-ID VarInts,
/// data as packed BE `i64`s (little-endian-within-long). The wire palette
/// keeps Anvil order, so Anvil indices are already wire indices.
fn encode_indirect_section(ids: &[u32], anvil: &[i64], out: &mut Vec<u8>) {
    let palette_len = ids.len().max(1);
    let mut bpe = 4u32;
    while (1u32 << bpe) < palette_len as u32 {
        bpe += 1;
    }
    let anvil_bpe = bits_for_palette(palette_len);
    let mut nonzero = 0u16;
    for i in 0..4096 {
        if ids.get(read_packed(anvil, anvil_bpe, i) as usize).copied().unwrap_or(0) != 0 {
            nonzero += 1;
        }
    }
    out.extend_from_slice(&nonzero.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // fluid count unknown; 0
    out.push(bpe as u8);
    write_varint(palette_len as u32, out);
    for id in ids {
        write_varint(*id, out);
    }
    // Copy indices, repacking when Anvil BPE differs from wire BPE.
    let per = 64 / bpe as usize;
    let longs = 4096_usize.div_ceil(per);
    for l in 0..longs {
        let mut word = 0u64;
        for k in 0..per {
            let i = l * per + k;
            if i >= 4096 {
                break;
            }
            let idx = read_packed(anvil, anvil_bpe, i) as u64;
            word |= idx << (k as u32 * bpe);
        }
        out.extend_from_slice(&(word as i64).to_be_bytes());
    }
}

/// Block count 0 + fluid 0 + BPE 0 air + single-plains biome: 8 bytes.
fn encode_air_section(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
}

/// BPE 0 container: the whole section is one palette entry.
fn encode_single_valued(ids: &[u32], out: &mut Vec<u8>) {
    let id = ids.first().copied().unwrap_or(0);
    let nonzero = if id == 0 { 0u16 } else { 4096u16 };
    out.extend_from_slice(&nonzero.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // fluid count
    out.push(0x00);
    write_varint(id, out);
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

/// Bits per entry the Anvil file uses for a palette of this length.
fn bits_for_palette(len: usize) -> u32 {
    if len <= 1 {
        return 0;
    }
    let mut bpe = 1u32;
    while (1usize << bpe) < len {
        bpe += 1;
    }
    bpe.max(4)
}

/// Read entry `i` from longs packed `bpe` bits per entry, LSB-first.
fn read_packed(longs: &[i64], bpe: u32, i: usize) -> u32 {
    if bpe == 0 {
        return 0;
    }
    let per_long = 64 / bpe as usize;
    let long_idx = i / per_long;
    let bit_idx = (i % per_long) as u32 * bpe;
    let word = longs.get(long_idx).copied().unwrap_or(0) as u64;
    ((word >> bit_idx) & ((1u64 << bpe) - 1)) as u32
}

fn write_varint(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_boundaries() {
        for v in [0u32, 1, 127, 128, 255, 16383, 2097151, 35722] {
            let mut out = Vec::new();
            write_varint(v, &mut out);
            let mut x = v;
            let mut i = 0;
            loop {
                let b = out[i];
                assert_eq!(b & 0x7F, (x & 0x7F) as u8);
                x >>= 7;
                i += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            assert_eq!(x, 0);
        }
    }

    #[test]
    fn packed_roundtrip_at_bpe_4_and_5() {
        // BPE 5 hits the wiki padding case: floor(64/5)=12 entries/long.
        for bpe in [1u32, 2, 4, 5, 6, 8] {
            let mut grid = [0u32; 4096];
            let mask = (1u32 << bpe) - 1;
            for (i, g) in grid.iter_mut().enumerate() {
                *g = (i as u32).wrapping_mul(2654435761u32) & mask;
            }
            let per_long = 64 / bpe as usize;
            let longs = 4096_usize.div_ceil(per_long);
            let mut words = vec![0i64; longs];
            for (i, v) in grid.iter().enumerate() {
                let word = &mut words[i / per_long];
                let mut w = *word as u64;
                w |= (*v as u64) << ((i % per_long) as u32 * bpe);
                *word = w as i64;
            }
            for (i, want) in grid.iter().enumerate() {
                assert_eq!(read_packed(&words, bpe, i), *want, "bpe={bpe} i={i}");
            }
        }
    }

    #[test]
    fn single_valued_encodes_air() {
        // 2 block-count + 2 fluid + BPE 0 + varint id; caller appends biome.
        let mut out = Vec::new();
        encode_single_valued(&[0], &mut out);
        assert_eq!(out, vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let mut out = Vec::new();
        encode_single_valued(&[1], &mut out);
        assert_eq!(out, vec![0x10, 0x00, 0x00, 0x00, 0x00, 0x01]);
    }
}
