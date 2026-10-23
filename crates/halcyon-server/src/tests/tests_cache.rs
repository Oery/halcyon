use super::*;
use mc_protocol::types::Heightmap;

#[test]
fn configuration_preserves_tags_and_identifies_supported_movement_blocks_and_fluids() {
    futures_lite::future::block_on(async {
        let cache = PacketCache::new().await.unwrap();
        let mut found = false;
        for frame in &cache.configuration {
            let mut payload = frame.as_ref();
            VarInt::read(&mut payload).unwrap();
            if VarInt::read(&mut payload).unwrap().0 != UpdateTags::ID.0 {
                continue;
            }
            let tags = UpdateTags::decode_payload(&mut payload).unwrap();
            assert!(payload.is_empty());
            assert!(tags.registries.iter().any(|r| r.registry == "minecraft:damage_type"));
            let blocks = tags.registries.iter().find(|r| r.registry == "minecraft:block").unwrap();
            assert!(blocks.tags.iter().any(|tag| tag.name == "minecraft:infiniburn_overworld"));
            let flow =
                blocks.tags.iter().find(|tag| tag.name == "minecraft:blocks_fluid_flow").unwrap();
            assert!(flow.entries.iter().any(|id| id.0 == 1)); // stone, a block ID rather than state ID
            assert!(!flow.entries.iter().any(|id| id.0 == 0)); // air
            let climbable: Vec<_> =
                blocks.tags.iter().filter(|tag| tag.name == "minecraft:climbable").collect();
            assert_eq!(climbable.len(), 1);
            assert_eq!(
                climbable[0].entries.iter().map(|id| id.0).collect::<Vec<_>>(),
                [266, 416, 927, 968, 969, 970, 971, 1225, 1226] // ladder, scaffolding and vine IDs
            );
            let fluid = tags.registries.iter().find(|r| r.registry == "minecraft:fluid").unwrap();
            let water = fluid.tags.iter().find(|tag| tag.name == "minecraft:water").unwrap();
            assert_eq!(water.entries.iter().map(|id| id.0).collect::<Vec<_>>(), [2, 1]);
            let lava = fluid.tags.iter().find(|tag| tag.name == "minecraft:lava").unwrap();
            assert_eq!(lava.entries.iter().map(|id| id.0).collect::<Vec<_>>(), [4, 3]);
            assert_eq!(
                tags.registries.iter().filter(|r| r.registry == "minecraft:fluid").count(),
                1
            );
            found = true;
        }
        assert!(found);
    });
}

#[test]
fn shared_chunk_parts_match_the_packet_codec() {
    futures_lite::future::block_on(async {
        let data = [1, 2, 3];
        let heights = [5; 37];
        let packet = ChunkData {
            chunk_x: -10,
            chunk_z: 20,
            heightmaps: vec![Heightmap { kind: 4, data: heights.to_vec() }],
            data: &data,
            block_entities: vec![],
            light: LightData::fullbright(),
        };
        let expected = encode(&packet).await.unwrap();
        let mut suffix = Vec::new();
        VarInt(0).write(&mut suffix).await.unwrap();
        packet.light.write(&mut suffix).await.unwrap();
        let (header, offset) =
            chunk_header(-10, 20, &heights, data.len(), suffix.len()).await.unwrap();
        assert_eq!(&header[offset..offset + 4], &(-10i32).to_be_bytes());
        let actual = [header.as_ref(), &data, &suffix].concat();
        assert_eq!(actual, expected.as_ref());
    });
}
