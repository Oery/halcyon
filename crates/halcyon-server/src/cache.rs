//! Immutable wire encodings built before accepting connections. Chunk section
//! bytes and the 104 KiB lighting suffix stay shared, rather than copied into
//! a complete cached frame for every chunk.
use crate::error::Error;
use mc_protocol::data::registries;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::*;
use mc_protocol::types::{KnownPack, LightData, RegistryEntry, Tag, TaggedRegistry};
use mc_protocol::{PacketWriter, VarInt};
use std::collections::HashMap;
use std::sync::Arc;

pub struct EncodedChunk {
    pub header: Box<[u8]>,
    pub data: Arc<[u8]>,
}

pub struct PacketCache {
    pub configuration: Vec<Box<[u8]>>,
    pub chunks: HashMap<(i32, i32), EncodedChunk>,
    pub empty: EncodedChunk,
    pub empty_coordinates: usize,
    pub chunk_suffix: Box<[u8]>,
}

async fn encode<'p, P: Payload<'p>>(packet: &P) -> Result<Box<[u8]>, Error> {
    let mut bytes = Vec::new();
    packet.write_packet(&mut bytes).await?;
    Ok(bytes.into_boxed_slice())
}

async fn chunk_header(
    x: i32,
    z: i32,
    heights: &[i64],
    data_len: usize,
    suffix_len: usize,
) -> Result<(Box<[u8]>, usize), Error> {
    let mut body = Vec::with_capacity(320);
    x.write(&mut body).await?;
    z.write(&mut body).await?;
    VarInt(1).write(&mut body).await?; // one MOTION_BLOCKING heightmap
    4u8.write(&mut body).await?;
    VarInt(heights.len() as i32).write(&mut body).await?;
    for height in heights {
        height.write(&mut body).await?;
    }
    VarInt(data_len as i32).write(&mut body).await?;
    let length = ChunkData::ID.body_len() + body.len() + data_len + suffix_len;
    let mut header = Vec::with_capacity(body.len() + 6);
    VarInt(i32::try_from(length).map_err(|_| mc_protocol::error::EncodeError::InvalidLength)?)
        .write(&mut header)
        .await?;
    ChunkData::ID.write(&mut header).await?;
    let coordinates = header.len();
    header.extend_from_slice(&body);
    Ok((header.into_boxed_slice(), coordinates))
}

impl PacketCache {
    pub async fn new(world: &halcyon_world::World) -> Result<Self, Error> {
        let mut configuration = Vec::new();
        configuration.push(
            encode(&KnownPacks {
                packs: vec![KnownPack { namespace: "minecraft", id: "core", version: "26.3" }],
            })
            .await?,
        );
        for (registry, entries) in registries::all() {
            configuration.push(
                encode(&RegistryData {
                    registry,
                    entries: entries
                        .into_iter()
                        .map(|(id, data)| RegistryEntry { id, data: Some(data) })
                        .collect(),
                })
                .await?,
            );
        }
        configuration.push(encode(&FeatureFlags { flags: vec!["minecraft:vanilla"] }).await?);
        let mut tagged: Vec<TaggedRegistry> = registries::all_tags()
            .into_iter()
            .map(|(registry, tags)| TaggedRegistry {
                registry,
                tags: tags
                    .into_iter()
                    .map(|(name, entries)| Tag {
                        name,
                        entries: entries.into_iter().map(VarInt).collect(),
                    })
                    .collect(),
            })
            .collect();
        tagged.extend(registries::builtin_empty_tags().into_iter().map(|(registry, tags)| {
            TaggedRegistry {
                registry,
                tags: tags.into_iter().map(|name| Tag { name, entries: vec![] }).collect(),
            }
        }));
        // Client movement must recognize the same supported blocks as the owner.
        // Keep the existing block tags in this group.
        tagged
            .iter_mut()
            .find(|registry| registry.registry == "minecraft:block")
            .expect("configuration includes built-in block tags")
            .tags
            .extend([
                Tag {
                    name: "minecraft:blocks_fluid_flow",
                    entries: mc_protocol::data::builtin_tags::BLOCKS_FLUID_FLOW
                        .iter()
                        .copied()
                        .map(VarInt)
                        .collect(),
                },
                Tag {
                    name: "minecraft:climbable",
                    // Built-in block IDs verified against the official 26.3 client:
                    // ladder, vine, weeping/twisting vines and plants, cave vines and plant.
                    // Scaffolding remains unsupported by the owner.
                    entries: [266, 416, 968, 969, 970, 971, 1225, 1226]
                        .into_iter()
                        .map(VarInt)
                        .collect(),
                },
            ]);
        // Built-in fluid registry IDs, verified against the official 26.3 client:
        // empty=0, flowing_water=1, water=2, flowing_lava=3, lava=4.
        // Missing fluid tags make the vanilla client treat water as air.
        tagged.push(TaggedRegistry {
            registry: "minecraft:fluid",
            tags: vec![
                Tag { name: "minecraft:water", entries: vec![VarInt(2), VarInt(1)] },
                Tag { name: "minecraft:lava", entries: vec![VarInt(4), VarInt(3)] },
            ],
        });
        configuration.push(encode(&UpdateTags { registries: tagged }).await?);
        configuration.push(encode(&FinishConfiguration).await?);

        let mut suffix = Vec::new();
        VarInt(0).write(&mut suffix).await?; // no block entities
        LightData::fullbright().write(&mut suffix).await?;
        let mut chunks = HashMap::with_capacity(world.chunk_count());
        for chunk in world.iter() {
            let (header, _) =
                chunk_header(chunk.x, chunk.z, &chunk.heights[..], chunk.data.len(), suffix.len())
                    .await?;
            chunks
                .insert((chunk.x, chunk.z), EncodedChunk { header, data: Arc::clone(&chunk.data) });
        }
        let data: Arc<[u8]> = Arc::from([0; 192]);
        let (header, empty_coordinates) =
            chunk_header(0, 0, &[0; 37], data.len(), suffix.len()).await?;
        Ok(Self {
            configuration,
            chunks,
            empty: EncodedChunk { header, data },
            empty_coordinates,
            chunk_suffix: suffix.into_boxed_slice(),
        })
    }
}

#[cfg(test)]
#[path = "tests/tests_cache.rs"]
mod tests;
