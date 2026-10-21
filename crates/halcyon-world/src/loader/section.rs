//! Map Anvil palettes into retained states and pre-encoded chunk sections.

use crate::blockstates::{block_state_id, canonical_key};
use crate::disk_nbt::DiskNbt;
use crate::{BlockStateId, LoadError, Section};

/// Encode one Anvil section into wire `Chunk Section` bytes and append.
pub(super) fn encode_section(
    chunk: &str,
    section: &DiskNbt,
    out: &mut Vec<u8>,
    remapped: &mut usize,
) -> Result<Section, LoadError> {
    let Some(block_states) = section.get("block_states") else {
        encode_air_section(out);
        return Ok(Section::default());
    };
    let Some(palette) = block_states.get("palette").and_then(|v| v.as_list()) else {
        encode_air_section(out);
        return Ok(Section::default());
    };
    if palette.is_empty() {
        return Err(LoadError::BadNbt {
            chunk: chunk.to_owned(),
            reason: "empty block palette".to_owned(),
        });
    }

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
                return Err(LoadError::UnknownBlockState { chunk: chunk.to_owned(), state: key });
            }
        }
    }

    // Biome palette ignored: every section encodes single plains (index 0)
    // because the synced `worldgen/biome` registry only contains plains.
    *remapped += 1;

    let terrain = if let Some(data) = block_states.get("data").and_then(|v| v.as_long_array()) {
        let bpe = bits_for_palette(ids.len());
        let required = if bpe == 0 {
            0
        } else {
            4096_usize.div_ceil(64 / bpe as usize)
        };
        if data.len() < required {
            return Err(LoadError::BadNbt {
                chunk: chunk.to_owned(),
                reason: "truncated packed block states".to_owned(),
            });
        }
        let mut states = Box::new([0; 4096]);
        for (i, state) in states.iter_mut().enumerate() {
            let index = read_packed(data, bpe, i) as usize;
            let id = ids.get(index).ok_or_else(|| LoadError::BadNbt {
                chunk: chunk.to_owned(),
                reason: format!("block palette index {index} outside {} entries", ids.len()),
            })?;
            *state = *id as BlockStateId;
        }
        encode_indirect_section(&ids, data, out);
        Section::from_states(states)
    } else {
        if ids.len() != 1 {
            return Err(LoadError::BadNbt {
                chunk: chunk.to_owned(),
                reason: "multi-entry block palette has no data".to_owned(),
            });
        }
        encode_single_valued(&ids, out);
        Section::Uniform(ids[0] as BlockStateId)
    };
    // Biome container: always single plains (BPE 0 + id 0 = 2 bytes).
    out.extend_from_slice(&[0x00, 0x00]);
    Ok(terrain)
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
pub(super) fn encode_air_section(out: &mut Vec<u8>) {
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

#[cfg(test)]
mod tests;
