use super::*;

#[test]
#[ignore = "loads a local map and reports hardware-specific component measurements"]
fn terrain_world_measurement() {
    let path = std::env::var_os("HALCYON_TERRAIN_WORLD").unwrap_or_else(|| "worlds/lobby".into());
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
