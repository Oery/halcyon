//! Owner-only sparse terrain edits and bounded sand/gravel simulation.
use halcyon_world::{
    BlockStateId, BlockView, MAX_BLOCK_Y, MIN_BLOCK_Y, Surface, World, block_properties,
    motion_info, state_info, try_state_info,
};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use uuid::Uuid;

pub type BlockPos = [i32; 3];
pub type Column = (i32, i32);
pub const MAX_FALLING: usize = 1024;
const MAX_CHANGED: usize = 16_384;
const MAX_COLUMN_CHANGES: usize = 1024;
const MAX_SCHEDULED: usize = 16_384;
const STARTS_PER_STEP: usize = 64;
const HALF_WIDTH: f64 = 0.98f32 as f64 / 2.0;
const HEIGHT: f64 = 0.98f32 as f64;

pub fn column(pos: BlockPos) -> Column {
    (pos[0].div_euclid(16), pos[2].div_euclid(16))
}
pub fn pack(pos: BlockPos) -> i64 {
    ((i64::from(pos[0]) & 0x3ffffff) << 38)
        | ((i64::from(pos[2]) & 0x3ffffff) << 12)
        | (i64::from(pos[1]) & 0xfff)
}
pub fn unpack(pos: i64) -> BlockPos {
    [(pos >> 38) as i32, (pos << 52 >> 52) as i32, (pos << 26 >> 38) as i32]
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("invalid or unsupported terrain edit")]
    Unsupported,
    #[error("arena terrain capacity exhausted")]
    Capacity,
    #[error("arena action queue is full or closed")]
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
pub struct BlockChange {
    pub state: BlockStateId,
    pub revision: u64,
}
#[derive(Default)]
pub struct ChangedColumn {
    pub states: HashMap<BlockPos, BlockChange>,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct FallingBlock {
    pub id: i32,
    pub uuid: Uuid,
    pub state: BlockStateId,
    pub position: [f64; 3],
    pub velocity: f64,
    pub revision: u64,
    age: u16,
}
impl FallingBlock {
    pub fn column(&self) -> Column {
        ((self.position[0].floor() as i32) >> 4, (self.position[2].floor() as i32) >> 4)
    }
}

#[derive(Default)]
pub struct Arena {
    pub columns: HashMap<Column, ChangedColumn>,
    pub falling: Vec<FallingBlock>,
    by_id: HashMap<i32, usize>,
    pub landings: HashMap<i32, (BlockPos, BlockStateId)>,
    scheduled: BinaryHeap<Reverse<(u64, BlockPos)>>,
    scheduled_at: HashMap<BlockPos, u64>,
    changed: usize,
    tick: u64,
    pub revision: u64,
}

pub struct View<'a> {
    pub base: &'a World,
    pub arena: &'a Arena,
}
impl BlockView for View<'_> {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        self.arena
            .columns
            .get(&(x.div_euclid(16), z.div_euclid(16)))
            .and_then(|c| c.states.get(&[x, y, z]))
            .map(|s| s.state)
            .or_else(|| self.base.block_at(x, y, z))
    }
    fn contains_column(&self, x: i32, z: i32) -> bool {
        self.base.contains_column(x, z)
    }
}

impl Arena {
    #[cfg(test)]
    pub fn capacity_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        self.columns.capacity() * size_of::<(Column, ChangedColumn)>()
            + self
                .columns
                .values()
                .map(|c| c.states.capacity() * size_of::<(BlockPos, BlockChange)>())
                .sum::<usize>()
            + self.falling.capacity() * size_of::<FallingBlock>()
            + self.by_id.capacity() * size_of::<(i32, usize)>()
            + self.landings.capacity() * size_of::<(i32, (BlockPos, BlockStateId))>()
            + self.scheduled.capacity() * size_of::<Reverse<(u64, BlockPos)>>()
            + self.scheduled_at.capacity() * size_of::<(BlockPos, u64)>()
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn get_falling(&self, id: i32) -> Option<&FallingBlock> {
        self.by_id.get(&id).map(|&index| &self.falling[index])
    }
    pub fn initialize(&mut self, base: &World) -> Result<(), EditError> {
        // One cold scan. No per-tick world scan or per-player terrain copies.
        for chunk in base.iter() {
            for (section_index, section) in chunk.sections.iter().enumerate() {
                if let halcyon_world::Section::Uniform(id) = section
                    && !block_properties(*id).falls()
                {
                    continue;
                }
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            if !block_properties(section.block_at(x, y, z)).falls() {
                                continue;
                            }
                            let pos = [
                                chunk.x * 16 + x as i32,
                                MIN_BLOCK_Y + section_index as i32 * 16 + y as i32,
                                chunk.z * 16 + z as i32,
                            ];
                            if base
                                .block_at(pos[0], pos[1] - 1, pos[2])
                                .is_some_and(|s| block_properties(s).free())
                            {
                                if !fall_path_supported(&self.view(base), pos, None) {
                                    return Err(EditError::Unsupported);
                                }
                                self.schedule(pos, self.tick + 2)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn view<'a>(&'a self, base: &'a World) -> View<'a> {
        View { base, arena: self }
    }

    fn schedule(&mut self, pos: BlockPos, tick: u64) -> Result<(), EditError> {
        if self.scheduled_at.contains_key(&pos) {
            return Ok(());
        }
        if self.scheduled_at.len() == MAX_SCHEDULED {
            return Err(EditError::Capacity);
        }
        self.scheduled_at.insert(pos, tick);
        self.scheduled.push(Reverse((tick, pos)));
        Ok(())
    }

    pub fn edit(
        &mut self,
        base: &World,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Result<(), EditError> {
        if try_state_info(state).is_none()
            || motion_info(state).surface == Surface::Unsupported
            || base.block_at(pos[0], pos[1], pos[2]).is_none()
        {
            return Err(EditError::Unsupported);
        }
        if self.view(base).block_at(pos[0], pos[1], pos[2]) == Some(state) {
            return Ok(());
        }
        // Reserve scheduled work before changing authority; failed edits are atomic.
        let above = [pos[0], pos[1] + 1, pos[2]];
        let needs_self = block_properties(state).falls();
        let needs_above = self
            .view(base)
            .block_at(above[0], above[1], above[2])
            .is_some_and(|s| block_properties(s).falls());
        let proposed = Some((pos, state));
        if (needs_self && !fall_path_supported(&self.view(base), pos, proposed))
            || (needs_above && !fall_path_supported(&self.view(base), above, proposed))
            || ((block_properties(state).entity_motion_effects()
                || state_info(state).contextual_geometry())
                && self
                    .falling
                    .iter()
                    .any(|e| e.column() == column(pos) && e.position[1] > pos[1] as f64))
        {
            return Err(EditError::Unsupported);
        }
        let new_schedules = [(pos, needs_self), (above, needs_above)]
            .into_iter()
            .filter(|(p, needed)| *needed && !self.scheduled_at.contains_key(p))
            .count();
        if self.scheduled_at.len() + new_schedules > MAX_SCHEDULED || !self.room(pos, 1) {
            return Err(EditError::Capacity);
        }
        self.write(pos, state);
        if needs_self {
            self.schedule(pos, self.tick + 2)?;
        }
        if needs_above {
            self.schedule(above, self.tick + 2)?;
        }
        Ok(())
    }

    fn room(&self, pos: BlockPos, reserve: usize) -> bool {
        let col = column(pos);
        let entry = self.columns.get(&col);
        let new = usize::from(entry.is_none_or(|c| !c.states.contains_key(&pos)));
        self.changed + self.falling.len() + new + reserve <= MAX_CHANGED
            && entry.map_or(0, |c| c.states.len())
                + self.falling.iter().filter(|e| e.column() == col).count()
                + new
                + reserve
                <= MAX_COLUMN_CHANGES
    }

    fn write(&mut self, pos: BlockPos, state: BlockStateId) {
        self.revision += 1;
        let col = self.columns.entry(column(pos)).or_default();
        if col.states.insert(pos, BlockChange { state, revision: self.revision }).is_none() {
            self.changed += 1;
        }
        col.revision = self.revision;
    }

    pub fn step(&mut self, base: &World, last_id: &mut i32) {
        self.tick += 1;
        self.landings.clear();
        for _ in 0..STARTS_PER_STEP {
            let Some(&Reverse((due, pos))) = self.scheduled.peek() else {
                break;
            };
            if due > self.tick {
                break;
            }
            self.scheduled.pop();
            self.scheduled_at.remove(&pos);
            let view = self.view(base);
            let Some(state) =
                view.block_at(pos[0], pos[1], pos[2]).filter(|s| block_properties(*s).falls())
            else {
                continue;
            };
            if !view
                .block_at(pos[0], pos[1] - 1, pos[2])
                .is_some_and(|s| block_properties(s).free())
            {
                continue;
            }
            if !fall_path_supported(&view, pos, None) {
                continue;
            }
            let Some(id) = last_id.checked_add(1) else {
                let _ = self.schedule(pos, self.tick + 1);
                break;
            };
            if self.falling.len() == MAX_FALLING || !self.room(pos, 2) {
                let _ = self.schedule(pos, self.tick + 1);
                break;
            }
            *last_id = id;
            self.write(pos, 0);
            let above = [pos[0], pos[1] + 1, pos[2]];
            if self
                .view(base)
                .block_at(above[0], above[1], above[2])
                .is_some_and(|s| block_properties(s).falls())
            {
                let _ = self.schedule(above, self.tick + 2);
            }
            self.by_id.insert(id, self.falling.len());
            self.falling.push(FallingBlock {
                id,
                uuid: Uuid::new_v4(),
                state,
                position: [pos[0] as f64 + 0.5, pos[1] as f64, pos[2] as f64 + 0.5],
                velocity: 0.0,
                age: 0,
                revision: self.tick,
            });
        }
        let mut index = 0;
        while index < self.falling.len() {
            let entity = &self.falling[index];
            let wanted = entity.velocity - 0.04;
            let Some(dy) = clip_fall(&self.view(base), entity.position, wanted) else {
                index += 1;
                continue;
            };
            let entity = &mut self.falling[index];
            entity.age += 1;
            entity.position[1] += dy;
            entity.velocity = wanted * (0.98f32 as f64);
            entity.revision = self.tick;
            let expired = entity.age > 600
                || (entity.age > 100
                    && (entity.position[1] < MIN_BLOCK_Y as f64
                        || entity.position[1] >= MAX_BLOCK_Y as f64));
            let landed = dy > wanted + 1e-7;
            if landed || expired {
                let entity = self.falling.swap_remove(index);
                self.by_id.remove(&entity.id);
                if let Some(moved) = self.falling.get(index) {
                    self.by_id.insert(moved.id, index);
                }
                if landed {
                    let pos = entity.position.map(|n| n.floor() as i32);
                    let view = self.view(base);
                    let replaceable = view
                        .block_at(pos[0], pos[1], pos[2])
                        .is_some_and(|s| block_properties(s).replaceable());
                    let support = view
                        .block_at(pos[0], pos[1] - 1, pos[2])
                        .is_some_and(|s| !block_properties(s).free());
                    if replaceable && support {
                        self.write(pos, entity.state);
                        self.landings.insert(entity.id, (pos, entity.state));
                        // Notify a stack above the newly settled block as well.
                        let above = [pos[0], pos[1] + 1, pos[2]];
                        if self
                            .view(base)
                            .block_at(above[0], above[1], above[2])
                            .is_some_and(|s| block_properties(s).falls())
                        {
                            let _ = self.schedule(above, self.tick + 2);
                        }
                    }
                }
                self.revision += 1;
            } else {
                index += 1;
            }
        }
    }
}

/// Pure vertical falling blocks have a 0.98-wide box. Clip the entire swept
/// path, including fast falls, against the same versioned static geometry.
fn clip_fall(view: &impl BlockView, pos: [f64; 3], wanted: f64) -> Option<f64> {
    let mut dy = wanted;
    for x in (pos[0] - HALF_WIDTH).floor() as i32 - 1..=(pos[0] + HALF_WIDTH).floor() as i32 + 1 {
        for z in (pos[2] - HALF_WIDTH).floor() as i32 - 1..=(pos[2] + HALF_WIDTH).floor() as i32 + 1
        {
            for y in (pos[1] + wanted).floor() as i32 - 1..=(pos[1] + HEIGHT).floor() as i32 + 1 {
                let Some(id) = view.block_at(x, y, z) else {
                    continue;
                };
                let info = state_info(id);
                if info.contextual_geometry()
                    && pos[0] + HALF_WIDTH > x as f64
                    && pos[0] - HALF_WIDTH < x as f64 + 1.0
                    && pos[2] + HALF_WIDTH > z as f64
                    && pos[2] - HALF_WIDTH < z as f64 + 1.0
                    && pos[1] + HEIGHT > y as f64
                    && pos[1] + wanted < y as f64 + 1.0
                {
                    return None;
                }
                if info.contextual_geometry() {
                    continue;
                }
                for shape in info.collision_boxes() {
                    if pos[0] + HALF_WIDTH <= x as f64 + shape.min[0] + 1e-7
                        || pos[0] - HALF_WIDTH >= x as f64 + shape.max[0] - 1e-7
                        || pos[2] + HALF_WIDTH <= z as f64 + shape.min[2] + 1e-7
                        || pos[2] - HALF_WIDTH >= z as f64 + shape.max[2] - 1e-7
                    {
                        continue;
                    }
                    let top = y as f64 + shape.max[1];
                    if pos[1] >= top - 1e-7 {
                        dy = dy.max((top - pos[1]).min(0.0));
                    }
                }
            }
        }
    }
    Some(dy)
}

/// Validate the bounded vertical route before removing its source. Contextual
/// geometry and entity-specific media are unavailable until their physics is
/// implemented; ordinary static fluids do not alter this entity's gravity.
fn fall_path_supported(
    view: &impl BlockView,
    pos: BlockPos,
    edit: Option<(BlockPos, BlockStateId)>,
) -> bool {
    for y in (MIN_BLOCK_Y..=pos[1]).rev() {
        // Offset/contextual shapes can protrude from adjacent cells. Reject
        // those routes before spawning instead of consulting a representative
        // empty-context shape while the entity is already falling.
        for x in pos[0] - 1..=pos[0] + 1 {
            for z in pos[2] - 1..=pos[2] + 1 {
                let p = [x, y, z];
                let state = if let Some((changed, state)) = edit
                    && changed == p
                {
                    Some(state)
                } else {
                    view.block_at(x, y, z)
                };
                if state.is_some_and(|s| state_info(s).contextual_geometry()) {
                    return false;
                }
            }
        }
        if y == pos[1] {
            continue;
        }
        let p = [pos[0], y, pos[2]];
        let state = if let Some((changed, state)) = edit
            && changed == p
        {
            Some(state)
        } else {
            view.block_at(p[0], p[1], p[2])
        };
        let Some(state) = state else {
            return false;
        };
        if block_properties(state).entity_motion_effects()
            || state_info(state).contextual_geometry()
        {
            return false;
        }
        if !state_info(state).collision_boxes().is_empty() {
            return true;
        }
    }
    true
}

#[cfg(test)]
pub(crate) fn test_world() -> World {
    use halcyon_world::{LoadedWorld, SECTION_COUNT, Section, loader::LoadedChunk};
    let mut sections =
        Box::new(std::array::from_fn::<_, SECTION_COUNT, _>(|_| Section::Uniform(0)));
    sections[7] = Section::Uniform(1);
    World::from_loaded(LoadedWorld {
        chunks: vec![LoadedChunk {
            x: 0,
            z: 0,
            data: vec![],
            heights: [0; 37],
            wire_bytes: 0,
            sections,
        }],
        spawn: (8.5, 64.0, 8.5, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    })
}

#[cfg(test)]
mod tests;
