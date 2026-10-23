//! Typed, revisioned block entities. Encodings are shared across observers.
use crate::arena::{BlockPos, Column, EditError, FastMap, column, pack};
pub const MAX_PLAYING: usize = 256;
use futures_lite::future::block_on;
use halcyon_world::blocks::entities::{
    BlockEntityData as ImportedData, MAX_BLOCK_ENTITIES_PER_COLUMN, Sign, SignFace,
};
use halcyon_world::{BlockStateId, World, blocks, disk_nbt::DiskNbt};
use mc_protocol::data::{items::ITEMS, registries};
use mc_protocol::packets::{
    Payload,
    client::{BlockEntityData, BlockUpdate, WorldEvent},
};
use mc_protocol::types::{ItemStack, Nbt};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;

#[derive(Clone)]
pub enum EntityData {
    Sign(Arc<Sign>),
    Jukebox(ItemStack),
}

pub struct EntityRecord {
    pub block: BlockStateId,
    pub data: Option<EntityData>,
    pub revision: u64,
    pub bytes: Rc<[u8]>,
    pub frames: usize,
}

#[derive(Default)]
pub struct EntityColumn {
    pub records: FastMap<BlockPos, EntityRecord>,
    pub revision: u64,
}

#[derive(Clone, Copy)]
pub struct Playback {
    pub song: i32,
    pub until: u64,
}

#[derive(Default)]
pub struct BlockEntities {
    pub columns: FastMap<Column, EntityColumn>,
    pub playing: FastMap<BlockPos, Playback>,
    revision: u64,
}

pub struct Song {
    pub item: i32,
    pub registry: i32,
    pub ticks: u64,
}

pub fn songs() -> &'static [Song] {
    static SONGS: OnceLock<Box<[Song]>> = OnceLock::new();
    SONGS.get_or_init(|| {
        registries::jukebox_song()
            .into_iter()
            .enumerate()
            .filter_map(|(registry, (name, data))| {
                let name = name.strip_prefix("minecraft:")?;
                let item = ITEMS.iter().position(|(item, ..)| {
                    item.strip_prefix("minecraft:music_disc_") == Some(name)
                })? as i32;
                let Nbt::Compound(fields) = data else {
                    return None;
                };
                let seconds = fields.iter().find_map(|(key, value)| {
                    if *key == "length_in_seconds" {
                        if let Nbt::Float(seconds) = value {
                            Some(*seconds)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })?;
                Some(Song {
                    item,
                    registry: registry as i32,
                    ticks: (f64::from(seconds) * 20.0).ceil() as u64,
                })
            })
            .collect()
    })
}

impl BlockEntities {
    #[cfg(test)]
    pub fn capacity_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        self.columns.capacity() * size_of::<(Column, EntityColumn)>()
            + self.playing.capacity() * size_of::<(BlockPos, Playback)>()
            + self
                .columns
                .values()
                .map(|col| {
                    col.records.capacity() * size_of::<(BlockPos, EntityRecord)>()
                        + col
                            .records
                            .values()
                            .map(|record| {
                                record.bytes.len()
                                    + match &record.data {
                                        Some(EntityData::Sign(sign)) => sign.payload_bytes(),
                                        _ => 0,
                                    }
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
    pub fn record(&self, pos: BlockPos) -> Option<&EntityRecord> {
        self.columns.get(&column(pos))?.records.get(&pos)
    }

    pub fn initialize(&mut self, world: &World) -> Result<(), EditError> {
        for chunk in world.iter() {
            for entry in chunk.block_entities.iter() {
                let data = match &entry.data {
                    ImportedData::Sign(sign) => EntityData::Sign(sign.clone()),
                    ImportedData::Jukebox { disc } => EntityData::Jukebox(
                        disc.as_ref()
                            .and_then(|name| ITEMS.iter().position(|(item, ..)| item == name))
                            .and_then(|id| ItemStack::plain(id as i32, 1).ok())
                            .unwrap_or_default(),
                    ),
                };
                let pos = entry.pos;
                let state = world.block_at(pos[0], pos[1], pos[2]).ok_or(EditError::Unsupported)?;
                self.store(pos, state, Some(data), false)?;
            }
        }
        Ok(())
    }

    pub fn room(&self, pos: BlockPos) -> bool {
        self.columns.get(&column(pos)).is_none_or(|c| {
            c.records.contains_key(&pos) || c.records.len() < MAX_BLOCK_ENTITIES_PER_COLUMN
        })
    }

    /// Called only after terrain transaction preflight; stale block entities become tombstones.
    pub fn changed(&mut self, pos: BlockPos, state: BlockStateId) {
        let name = blocks::metadata(state).name;
        let previous = self.record(pos);
        let was_jukebox = previous.is_some_and(|r| matches!(r.data, Some(EntityData::Jukebox(_))));
        let data = if name.ends_with("_sign") {
            match previous {
                Some(record) if blocks::metadata(record.block).name == name => record.data.clone(),
                _ => Some(EntityData::Sign(Arc::default())),
            }
        } else if name == "minecraft:jukebox" {
            match previous {
                Some(record) if blocks::metadata(record.block).name == name => record.data.clone(),
                _ => Some(EntityData::Jukebox(ItemStack::default())),
            }
        } else {
            None
        };
        if data.is_none() && previous.is_none() {
            return;
        }
        if name != "minecraft:jukebox" {
            self.playing.remove(&pos);
        }
        self.store(pos, state, data, was_jukebox)
            .expect("bounded, validated block-entity encoding");
    }

    fn store(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        data: Option<EntityData>,
        stop: bool,
    ) -> Result<(), EditError> {
        if !self.room(pos) {
            return Err(EditError::Capacity);
        }
        let mut bytes = Vec::new();
        let mut frames = 1;
        block_on(
            BlockUpdate { location: pack(pos), block_state: i32::from(state) }
                .write_packet(&mut bytes),
        )
        .map_err(|_| EditError::Unsupported)?;
        match &data {
            Some(EntityData::Sign(sign)) => {
                let kind = if blocks::metadata(state).name.ends_with("_hanging_sign") {
                    8
                } else {
                    7
                };
                let nbt = Nbt::Compound(vec![
                    ("front_text", face(&sign.front)),
                    ("back_text", face(&sign.back)),
                    ("is_waxed", Nbt::Bool(true)),
                ]);
                block_on(
                    BlockEntityData { location: pack(pos), kind, data: nbt }
                        .write_packet(&mut bytes),
                )
                .map_err(|_| EditError::Unsupported)?;
                frames += 1;
            }
            Some(EntityData::Jukebox(stack)) => {
                let data = if stack.is_empty() {
                    Nbt::Compound(vec![])
                } else {
                    Nbt::Compound(vec![(
                        "RecordItem",
                        Nbt::Compound(vec![
                            ("id", Nbt::Str(ITEMS[stack.item_id() as usize].0)),
                            ("count", Nbt::Int(1)),
                        ]),
                    )])
                };
                block_on(
                    BlockEntityData { location: pack(pos), kind: 4, data }.write_packet(&mut bytes),
                )
                .map_err(|_| EditError::Unsupported)?;
                frames += 1;
            }
            None => {}
        }
        if let Some(playback) = self.playing.get(&pos) {
            block_on(
                WorldEvent { event: 1010, location: pack(pos), data: playback.song, global: false }
                    .write_packet(&mut bytes),
            )
            .map_err(|_| EditError::Unsupported)?;
            frames += 1;
        } else if stop || matches!(data, Some(EntityData::Jukebox(_))) {
            block_on(
                WorldEvent { event: 1011, location: pack(pos), data: 0, global: false }
                    .write_packet(&mut bytes),
            )
            .map_err(|_| EditError::Unsupported)?;
            frames += 1;
        }
        self.revision += 1;
        let col = self.columns.entry(column(pos)).or_default();
        col.revision = self.revision;
        col.records.insert(
            pos,
            EntityRecord {
                block: state,
                data,
                revision: self.revision,
                bytes: Rc::from(bytes),
                frames,
            },
        );
        Ok(())
    }

    pub fn set_disc(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        stack: ItemStack,
        tick: u64,
        play: bool,
    ) -> Result<(), EditError> {
        if play
            && !stack.is_empty()
            && !self.playing.contains_key(&pos)
            && self.playing.len() >= MAX_PLAYING
        {
            return Err(EditError::Capacity);
        }
        self.playing.remove(&pos);
        if play && !stack.is_empty() {
            let song =
                songs().iter().find(|s| s.item == stack.item_id()).ok_or(EditError::Unsupported)?;
            self.playing.insert(pos, Playback { song: song.registry, until: tick + song.ticks });
        }
        self.store(pos, state, Some(EntityData::Jukebox(stack)), true)
    }

    pub fn step(&mut self, tick: u64) {
        let expired: Vec<_> =
            self.playing.iter().filter(|(_, p)| tick >= p.until).map(|(&pos, _)| pos).collect();
        for pos in expired {
            self.playing.remove(&pos);
            if let Some(record) = self.record(pos) {
                let (state, data) = (record.block, record.data.clone());
                self.store(pos, state, data, true).expect("validated jukebox encoding");
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_sign(&mut self, pos: BlockPos, sign: Sign) -> Result<(), EditError> {
        let sign = sign.sanitized().ok_or(EditError::Unsupported)?;
        let record = self
            .record(pos)
            .filter(|r| matches!(r.data, Some(EntityData::Sign(_))))
            .ok_or(EditError::Unsupported)?;
        self.store(pos, record.block, Some(EntityData::Sign(Arc::new(sign))), false)
    }
}

fn face(face: &SignFace) -> Nbt<'_> {
    Nbt::Compound(vec![
        ("messages", Nbt::List(face.messages.iter().map(text).collect())),
        ("color", Nbt::Str(&face.color)),
        ("has_glowing_text", Nbt::Bool(face.glowing)),
    ])
}
fn text(value: &DiskNbt) -> Nbt<'_> {
    match value {
        DiskNbt::Str(s) => Nbt::Str(s),
        DiskNbt::Byte(v) => Nbt::Byte(*v),
        DiskNbt::Int(v) => Nbt::Int(*v),
        DiskNbt::List(_, v) => Nbt::List(v.iter().map(text).collect()),
        DiskNbt::Compound(v) => {
            Nbt::Compound(v.iter().map(|(k, v)| (k.as_str(), text(v))).collect())
        }
        _ => Nbt::Str(""),
    }
}
