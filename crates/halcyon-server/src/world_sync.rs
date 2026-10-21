//! Bounded, delivered baselines for terrain patches and falling entities.
use crate::arena::{Arena, BlockPos, Column, FallingBlock, pack};
use futures_lite::future::block_on;
use mc_protocol::{
    error::EncodeError,
    packets::{
        Payload,
        client::{
            AddTransientBlock, BlockUpdate, EntityPositionSync, RemoveEntities, SetEntityVelocity,
            SpawnFallingBlock,
        },
    },
    types::LpVec3,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
};

enum Commit {
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
        self.columns.capacity() * size_of::<(Column, u64)>()
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
    pub fn pending_work(&self) -> bool {
        self.work
    }
    pub fn frame_count(&self) -> usize {
        self.pending.as_ref().map_or(0, |p| p.frames)
    }
    pub fn base_sent(&mut self, column: Column) {
        self.columns.remove(&column);
        self.work = true;
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
        self.resident.remove(&column);
        self.rendered.retain(|_, e| e.column() != column);
        self.pending = None;
        self.tick = None;
        self.work = true;
    }
}

pub fn resync_cells(position: crate::lobby::Transform) -> impl Iterator<Item = BlockPos> {
    let [x, y, z] = [position.x, position.y, position.z].map(|n| n.floor() as i32);
    (x - 1..=x + 1).flat_map(move |bx| {
        (y - 1..=y + 2).flat_map(move |by| (z - 1..=z + 1).map(move |bz| [bx, by, bz]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::test_world;
    use mc_protocol::{packets::PacketWriter, types::VarInt};
    fn ids(bytes: &[u8]) -> Vec<i32> {
        let mut input = bytes;
        let mut ids = Vec::new();
        while !input.is_empty() {
            let n = VarInt::read(&mut input).unwrap().0 as usize;
            let mut body = &input[..n];
            ids.push(VarInt::read(&mut body).unwrap().0);
            input = &input[n..];
        }
        ids
    }
    #[test]
    fn patches_precede_spawns_and_delivered_baselines_survive_retry_and_chunk_resend() {
        let base = test_world();
        let mut arena = Arena::default();
        let mut id = 0;
        arena.edit(&base, [8, 70, 8], 127).unwrap();
        arena.step(&base, &mut id);
        arena.step(&base, &mut id);
        let mut observer = WorldObserver::default();
        let resident = HashSet::from([(0, 0)]);
        observer.reconcile(&arena, &resident);
        let patches = observer.next_frame(&arena).unwrap().unwrap();
        assert_eq!(ids(&patches), [8]);
        assert_eq!(observer.next_frame(&arena).unwrap().unwrap(), patches);
        assert!(observer.columns.is_empty());
        observer.sent();
        let spawn = observer.next_frame(&arena).unwrap().unwrap();
        assert_eq!(ids(&spawn), [1]);
        observer.sent();
        assert_eq!(observer.rendered.len(), 1);
        arena.step(&base, &mut id);
        observer.reconcile(&arena, &resident);
        let motion = observer.next_frame(&arena).unwrap().unwrap();
        assert_eq!(ids(&motion), [0x23, 0x67]);
        observer.sent();
        observer.base_sent((0, 0));
        assert!(observer.patch_column(&arena, (0, 0)).unwrap().is_some());
        observer.sent();
        for _ in 0..80 {
            arena.step(&base, &mut id);
        }
        observer.reconcile(&arena, &resident);
        let patch = observer.next_frame(&arena).unwrap().unwrap();
        assert_eq!(ids(&patch), [8]);
        observer.sent();
        let remove = observer.next_frame(&arena).unwrap().unwrap();
        assert_eq!(ids(&remove), [0x4e]);
        observer.sent();
        assert!(observer.rendered.is_empty());
        assert!(observer.next_frame(&arena).unwrap().is_none());
        observer.unloaded((0, 0));
        observer.reconcile(&arena, &HashSet::new());
        assert!(observer.next_frame(&arena).unwrap().is_none());
        observer.reconcile(&arena, &resident);
        assert!(observer.next_frame(&arena).unwrap().is_some());
    }
}
