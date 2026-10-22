use super::*;

fn test_section(names: &[&str], data: Option<Vec<i64>>) -> DiskNbt {
    let mut fields = vec![(
        "palette".to_owned(),
        DiskNbt::List(
            10,
            names
                .iter()
                .map(|name| {
                    DiskNbt::Compound(vec![("Name".to_owned(), DiskNbt::Str((*name).to_owned()))])
                })
                .collect(),
        ),
    )];
    if let Some(data) = data {
        fields.push(("data".to_owned(), DiskNbt::LongArray(data)));
    }
    DiskNbt::Compound(vec![("block_states".to_owned(), DiskNbt::Compound(fields))])
}

#[test]
fn retained_states_match_the_encoded_palette_and_indices() {
    let packed = vec![0x1010_1010_1010_1010i64; 256];
    let section = test_section(&["minecraft:air", "minecraft:stone"], Some(packed.clone()));
    let mut wire = Vec::new();
    let terrain = encode_section("test", &section, &mut wire, &mut 0).unwrap();
    assert_eq!(&wire[..8], &[0x08, 0, 0, 0, 4, 2, 0, 1]);
    assert_eq!(
        &wire[8..wire.len() - 2],
        packed.iter().flat_map(|word| word.to_be_bytes()).collect::<Vec<_>>()
    );
    for i in 0..4096 {
        assert_eq!(terrain.block_at(i & 15, i >> 8, (i >> 4) & 15), (i & 1) as u16);
    }
}

#[test]
fn unknown_or_malformed_palettes_do_not_become_gameplay_air() {
    let unknown = test_section(&["minecraft:not_a_block"], None);
    assert!(matches!(
        encode_section("test", &unknown, &mut vec![], &mut 0),
        Err(LoadError::UnknownBlockState { .. })
    ));
    for section in [
        test_section(&[], None),
        test_section(&["minecraft:air", "minecraft:stone"], None),
        test_section(&["minecraft:air", "minecraft:stone"], Some(vec![0; 255])),
        test_section(&["minecraft:air", "minecraft:stone"], Some(vec![2; 256])),
    ] {
        assert!(matches!(
            encode_section("test", &section, &mut vec![], &mut 0),
            Err(LoadError::BadNbt { .. })
        ));
    }
}

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

#[test]
fn imported_fluid_counts_match_retained_water_lava_and_waterlogged_states() {
    let water = block_state_id("minecraft:water[level=0]").unwrap();
    let lava = block_state_id("minecraft:lava[level=0]").unwrap();
    let wet_slab = block_state_id("minecraft:oak_slab[type=bottom,waterlogged=true]").unwrap();
    for id in [water, lava, wet_slab] {
        let mut wire = Vec::new();
        encode_single_valued(&[id], &mut wire);
        assert_eq!(&wire[..4], &[0x10, 0, 0x10, 0]);
    }
    let packed = vec![0x3210_3210_3210_3210i64; 256];
    let mut wire = Vec::new();
    encode_indirect_section(&[0, water, lava, wet_slab], &packed, &mut wire);
    assert_eq!(u16::from_be_bytes([wire[0], wire[1]]), 3072);
    assert_eq!(u16::from_be_bytes([wire[2], wire[3]]), 3072);
    // The complete loader must retain both counts when adding the biome data.
    let mut wire = Vec::new();
    let section = test_section(
        &["minecraft:air", "minecraft:kelp_plant"],
        Some(vec![0x1010_1010_1010_1010; 256]),
    );
    let terrain = encode_section("water", &section, &mut wire, &mut 0).unwrap();
    assert_eq!(u16::from_be_bytes([wire[2], wire[3]]), 2048);
    assert!(crate::state_info(terrain.block_at(1, 0, 0)).flags.has_fluid());
}
