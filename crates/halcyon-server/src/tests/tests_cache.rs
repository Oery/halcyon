use super::*;
use mc_protocol::types::Heightmap;

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
