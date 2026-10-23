//! Delivered terrain, block-entity, crack and falling-entity baselines.
use crate::arena::{Arena, BlockPos, Column, FallingBlock, pack};
use futures_lite::future::block_on;
use mc_protocol::{
    error::EncodeError,
    packets::{
        Payload,
        client::{
            AddTransientBlock, BlockUpdate, EntityPositionSync, RemoveEntities,
            SetBlockDestroyStage, SetEntityVelocity, SpawnFallingBlock, WorldEvent,
        },
    },
    types::LpVec3,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
};

enum Commit {
    BlockEntity(Column, u64, BlockPos, bool),
    Crack(i32, Option<crate::interaction::Crack>),
    Blocks(Column, u64),
    Entity(FallingBlock),
    Remove(i32),
}
struct Pending {
    bytes: Rc<[u8]>,
    frames: usize,
    commit: Commit,
}
#[derive(Default)]
pub struct WorldObserver {
    block_entities: HashMap<Column, u64>,
    cracks: HashMap<i32, crate::interaction::Crack>,
    crack_updates: VecDeque<i32>,
    playing: HashSet<BlockPos>,
    columns: HashMap<Column, u64>,
    rendered: HashMap<i32, FallingBlock>,
    resident: HashSet<Column>,
    tick: Option<u64>,
    waiting: VecDeque<i32>,
    removals: VecDeque<i32>,
    pending: Option<Pending>,
    admissions: usize,
    work: bool,
    cursor: i32,
}
impl WorldObserver {
    #[cfg(test)]
    pub fn capacity_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        self.block_entities.capacity() * size_of::<(Column, u64)>()
            + self.cracks.capacity() * size_of::<(i32, crate::interaction::Crack)>()
            + self.crack_updates.capacity() * size_of::<i32>()
            + self.playing.capacity() * size_of::<BlockPos>()
            + self.columns.capacity() * size_of::<(Column, u64)>()
            + self.rendered.capacity() * size_of::<(i32, FallingBlock)>()
            + self.resident.capacity() * size_of::<Column>()
            + (self.waiting.capacity() + self.removals.capacity()) * size_of::<i32>()
            + self.pending.as_ref().map_or(0, |p| p.bytes.len())
    }
    pub fn reconcile(&mut self, arena: &Arena, resident: &HashSet<Column>) {
        let tick_changed = self.tick != Some(arena.tick());
        if !tick_changed && self.resident == *resident {
            return;
        }
        if tick_changed {
            self.admissions = 0;
        }
        self.tick = Some(arena.tick());
        self.resident.clone_from(resident);
        self.columns.retain(|col, _| resident.contains(col));
        self.block_entities.retain(|col, _| resident.contains(col));
        self.reconcile_cracks(arena);
        self.removals = self
            .rendered
            .iter()
            .filter(|(id, e)| !resident.contains(&e.column()) || arena.get_falling(**id).is_none())
            .map(|(&id, _)| id)
            .collect();
        let mut ids: Vec<_> =
            arena.falling.iter().filter(|e| resident.contains(&e.column())).map(|e| e.id).collect();
        ids.sort_unstable_by_key(|id| (*id <= self.cursor, *id));
        self.waiting = ids.into();
        self.work = true;
    }

    /// Select changed miners once per owner tick, rather than searching both
    /// maps again for every packet. Only sent frames advance the baseline.
    fn reconcile_cracks(&mut self, arena: &Arena) {
        self.crack_updates.clear();
        self.crack_updates.extend(self.cracks.iter().filter_map(|(&id, old)| {
            let current = arena
                .cracks
                .get(&id)
                .filter(|c| self.resident.contains(&crate::arena::column(c.pos)));
            (current != Some(old)).then_some(id)
        }));
        self.crack_updates.extend(arena.cracks.iter().filter_map(|(&id, crack)| {
            (!self.cracks.contains_key(&id)
                && self.resident.contains(&crate::arena::column(crack.pos)))
            .then_some(id)
        }));
        // An in-flight crack can become obsolete before its write completes.
        // Recheck it after commit even if it has no delivered baseline yet.
        if let Some(Pending { commit: Commit::Crack(id, _), .. }) = &self.pending {
            self.crack_updates.push_back(*id);
        }
    }

    fn next_crack(&mut self, arena: &Arena) -> Result<Option<Pending>, EncodeError> {
        while let Some(id) = self.crack_updates.pop_front() {
            let old = self.cracks.get(&id).copied();
            let current = arena
                .cracks
                .get(&id)
                .filter(|c| self.resident.contains(&crate::arena::column(c.pos)))
                .copied();
            if old == current {
                continue;
            }
            let (crack, delivered) = match (old, current) {
                (Some(old), current) if current.is_none_or(|c| old.pos != c.pos) => {
                    if current.is_some() {
                        // Clear the old target before publishing this miner's new one.
                        self.crack_updates.push_front(id);
                    }
                    (old, None)
                }
                (_, Some(current)) => (current, Some(current)),
                _ => continue,
            };
            let mut bytes = Vec::new();
            block_on(
                SetBlockDestroyStage {
                    entity_id: id,
                    location: pack(crack.pos),
                    stage: delivered.map_or(255, |c| c.stage),
                }
                .write_packet(&mut bytes),
            )?;
            return Ok(Some(Pending {
                bytes: Rc::from(bytes),
                frames: 1,
                commit: Commit::Crack(id, delivered),
            }));
        }
        Ok(None)
    }

    pub fn pending_work(&self) -> bool {
        self.work
    }
    pub fn frame_count(&self) -> usize {
        self.pending.as_ref().map_or(0, |p| p.frames)
    }
    pub fn base_sent(&mut self, column: Column) {
        self.columns.remove(&column);
        self.block_entities.remove(&column);
        self.work = true;
    }

    pub fn patch_block_entities(
        &mut self,
        arena: &Arena,
        column: Column,
    ) -> Result<Option<Rc<[u8]>>, EncodeError> {
        if let Some(pending) = &self.pending {
            return Ok(Some(pending.bytes.clone()));
        }
        let Some(col) = arena.block_entities.columns.get(&column) else {
            return Ok(None);
        };
        let since = self.block_entities.get(&column).copied().unwrap_or(0);
        let Some((&pos, record)) =
            col.records.iter().filter(|(_, r)| r.revision > since).min_by_key(|(_, r)| r.revision)
        else {
            return Ok(None);
        };
        self.pending = Some(Pending {
            bytes: record.bytes.clone(),
            frames: record.frames,
            commit: Commit::BlockEntity(
                column,
                record.revision,
                pos,
                arena.block_entities.playing.contains_key(&pos),
            ),
        });
        self.work = true;
        Ok(self.pending.as_ref().map(|p| p.bytes.clone()))
    }

    pub fn patch_column(
        &mut self,
        arena: &Arena,
        column: Column,
    ) -> Result<Option<Rc<[u8]>>, EncodeError> {
        if let Some(p) = &self.pending {
            return Ok(Some(p.bytes.clone()));
        }
        let Some(changes) = arena.columns.get(&column) else {
            return Ok(None);
        };
        let since = self.columns.get(&column).copied().unwrap_or(0);
        if changes.revision <= since {
            return Ok(None);
        }
        let mut cells: Vec<_> = changes.states.iter().filter(|(_, s)| s.revision > since).collect();
        cells.sort_unstable_by_key(|(_, s)| s.revision);
        cells.truncate(16);
        let mut bytes = Vec::new();
        let mut revision = since;
        for &(pos, change) in &cells {
            block_on(
                BlockUpdate { location: pack(*pos), block_state: i32::from(change.state) }
                    .write_packet(&mut bytes),
            )?;
            revision = revision.max(change.revision);
        }
        if cells.is_empty() {
            return Ok(None);
        }
        self.pending = Some(Pending {
            bytes: Rc::from(bytes),
            frames: cells.len(),
            commit: Commit::Blocks(column, revision),
        });
        self.work = true;
        Ok(self.pending.as_ref().map(|p| p.bytes.clone()))
    }

    pub fn next_frame(&mut self, arena: &Arena) -> Result<Option<Rc<[u8]>>, EncodeError> {
        if let Some(p) = &self.pending {
            return Ok(Some(p.bytes.clone()));
        }
        // Existing columns stay shared. Only edited columns need patch scans.
        let column = arena
            .columns
            .iter()
            .find(|(col, changes)| {
                self.resident.contains(col)
                    && changes.revision > self.columns.get(col).copied().unwrap_or(0)
            })
            .map(|(&col, _)| col);
        if let Some(column) = column {
            return self.patch_column(arena, column);
        }
        let column = self
            .resident
            .iter()
            .find(|col| {
                arena.block_entities.columns.get(col).is_some_and(|data| {
                    data.revision > self.block_entities.get(col).copied().unwrap_or(0)
                })
            })
            .copied();
        if let Some(column) = column {
            return self.patch_block_entities(arena, column);
        }
        if let Some(crack) = self.next_crack(arena)? {
            self.pending = Some(crack);
            self.work = true;
            return Ok(self.pending.as_ref().map(|p| p.bytes.clone()));
        }
        if let Some(id) = self.removals.pop_front() {
            let mut bytes = Vec::new();
            let mut frames = 1;
            if let Some(&(pos, state)) = arena.landings.get(&id)
                && self.resident.contains(&crate::arena::column(pos))
            {
                block_on(
                    AddTransientBlock { location: pack(pos), block_state: i32::from(state) }
                        .write_packet(&mut bytes),
                )?;
                frames += 1;
            }
            block_on(RemoveEntities { entity_ids: &[id] }.write_packet(&mut bytes))?;
            self.pending =
                Some(Pending { bytes: Rc::from(bytes), frames, commit: Commit::Remove(id) });
        }
        while self.pending.is_none() {
            let Some(id) = self.waiting.pop_front() else {
                self.work = false;
                return Ok(None);
            };
            let Some(entity) = arena.get_falling(id) else {
                continue;
            };
            if !self.resident.contains(&entity.column()) {
                continue;
            }
            let mut bytes = Vec::new();
            let frames;
            if let Some(old) = self.rendered.get(&id) {
                if old.revision == entity.revision {
                    continue;
                }
                block_on(
                    EntityPositionSync {
                        entity_id: id,
                        x: entity.position[0],
                        y: entity.position[1],
                        z: entity.position[2],
                        yaw: 0.0,
                        pitch: 0.0,
                        on_ground: false,
                    }
                    .write_packet(&mut bytes),
                )?;
                block_on(
                    SetEntityVelocity {
                        entity_id: id,
                        velocity: LpVec3([0.0, entity.velocity, 0.0]),
                    }
                    .write_packet(&mut bytes),
                )?;
                frames = 2;
            } else {
                if self.admissions >= 16 {
                    continue;
                }
                self.admissions += 1;
                block_on(
                    SpawnFallingBlock {
                        entity_id: id,
                        uuid: entity.uuid,
                        entity_type: 52,
                        x: entity.position[0],
                        y: entity.position[1],
                        z: entity.position[2],
                        velocity: LpVec3([0.0, entity.velocity, 0.0]),
                        pitch: 0,
                        yaw: 0,
                        head_yaw: 0,
                        block_state: i32::from(entity.state),
                    }
                    .write_packet(&mut bytes),
                )?;
                frames = 1;
            }
            self.pending = Some(Pending {
                bytes: Rc::from(bytes),
                frames,
                commit: Commit::Entity(entity.clone()),
            });
            self.cursor = id;
        }
        self.work = true;
        Ok(self.pending.as_ref().map(|p| p.bytes.clone()))
    }

    pub fn sent(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending.commit {
            Commit::BlockEntity(col, revision, pos, playing) => {
                self.block_entities.insert(col, revision);
                if playing {
                    self.playing.insert(pos);
                } else {
                    self.playing.remove(&pos);
                }
            }
            Commit::Crack(id, crack) => {
                if let Some(crack) = crack {
                    self.cracks.insert(id, crack);
                } else {
                    self.cracks.remove(&id);
                }
            }
            Commit::Blocks(col, revision) => {
                self.columns.insert(col, revision);
            }
            Commit::Entity(entity) => {
                self.rendered.insert(entity.id, entity);
            }
            Commit::Remove(id) => {
                self.rendered.remove(&id);
            }
        }
    }
    pub fn unload_ids(&self, column: Column) -> Vec<i32> {
        self.rendered.values().filter(|e| e.column() == column).map(|e| e.id).collect()
    }
    pub fn unloaded(&mut self, column: Column) {
        self.columns.remove(&column);
        self.block_entities.remove(&column);
        self.cracks.retain(|_, c| crate::arena::column(c.pos) != column);
        self.crack_updates.clear();
        self.playing.retain(|p| crate::arena::column(*p) != column);
        self.resident.remove(&column);
        self.rendered.retain(|_, e| e.column() != column);
        self.pending = None;
        self.tick = None;
        self.work = true;
    }

    pub fn unload_effects(&self, column: Column) -> Result<Vec<Vec<u8>>, EncodeError> {
        fn append<'p>(
            batches: &mut Vec<Vec<u8>>,
            frames: &mut usize,
            packet: &impl Payload<'p>,
        ) -> Result<(), EncodeError> {
            if frames.is_multiple_of(crate::visibility::WRITE_SLICE_FRAMES) {
                batches.push(Vec::new());
            }
            block_on(packet.write_packet(batches.last_mut().expect("packet batch")))?;
            *frames += 1;
            Ok(())
        }
        let mut batches = Vec::new();
        let mut frames = 0;
        for pos in self.playing.iter().filter(|pos| crate::arena::column(**pos) == column) {
            append(
                &mut batches,
                &mut frames,
                &WorldEvent { event: 1011, location: pack(*pos), data: 0, global: false },
            )?;
        }
        for (&id, crack) in
            self.cracks.iter().filter(|(_, c)| crate::arena::column(c.pos) == column)
        {
            append(
                &mut batches,
                &mut frames,
                &SetBlockDestroyStage { entity_id: id, location: pack(crack.pos), stage: 255 },
            )?;
        }
        Ok(batches)
    }
}

pub fn resync_cells(position: crate::lobby::Transform) -> impl Iterator<Item = BlockPos> {
    let [x, y, z] = [position.x, position.y, position.z].map(|n| n.floor() as i32);
    (x - 1..=x + 1).flat_map(move |bx| {
        (y - 1..=y + 2).flat_map(move |by| (z - 1..=z + 1).map(move |bz| [bx, by, bz]))
    })
}

#[cfg(test)]
#[path = "tests/tests_world_sync.rs"]
mod tests;
