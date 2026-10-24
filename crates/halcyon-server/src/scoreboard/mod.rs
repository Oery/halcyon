//! Instance-owned scoreboard data. Connections publish only their selected objectives.
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use futures_lite::future::block_on;
use mc_protocol::PacketWriter;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::{
    ObjectiveAction, ObjectiveProperties, ScoreNumberFormat, SetObjective, SetScore,
};
use mc_protocol::types::Nbt;

use crate::instance::InstanceId;
use crate::text::MAX_TEXT_BYTES;
pub use crate::text::{Style, Text};
pub use mc_protocol::packets::client::ScoreRender;

mod delivery;
pub use delivery::ScoreboardObserver;

const MAX_OBJECTIVES: usize = 512;
const MAX_SCORES: usize = 16_384;
pub const MAX_SIDEBAR_LINES: usize = 15;

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Public game API; the built-in FFA uses blank numbers only.
pub enum NumberFormat {
    Blank,
    Styled(Style),
    Fixed(Text),
}

impl NumberFormat {
    fn wire(&self) -> ScoreNumberFormat<'_> {
        match self {
            Self::Blank => ScoreNumberFormat::Blank,
            Self::Styled(style) => ScoreNumberFormat::Styled(style.nbt()),
            Self::Fixed(text) => ScoreNumberFormat::Fixed(text.nbt()),
        }
    }

    fn validate(&self) -> Result<(), ScoreboardError> {
        match self {
            Self::Blank => Ok(()),
            Self::Styled(style) => Ok(style.validate()?),
            Self::Fixed(text) => Ok(text.validate()?),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectiveSpec {
    pub title: Text,
    pub render: ScoreRender,
    /// None uses the client's default number format.
    pub number_format: Option<NumberFormat>,
}

impl ObjectiveSpec {
    fn validate(&self) -> Result<(), ScoreboardError> {
        self.title.validate()?;
        if let Some(format) = &self.number_format {
            format.validate()?;
        }
        Ok(())
    }

    fn wire(&self) -> ObjectiveProperties<'_> {
        ObjectiveProperties {
            title: self.title.nbt(),
            render: self.render,
            number_format: self.number_format.as_ref().map(NumberFormat::wire),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Score {
    pub value: i32,
    pub display: Option<Text>,
    /// None inherits the objective's number format.
    pub number_format: Option<NumberFormat>,
}

impl Score {
    fn validate(&self) -> Result<(), ScoreboardError> {
        if let Some(text) = &self.display {
            text.validate()?;
        }
        if let Some(format) = &self.number_format {
            format.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectiveId {
    instance: InstanceId,
    serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Public game API; the built-in FFA displays a sidebar only.
pub enum DisplaySlot {
    PlayerList = 0,
    Sidebar = 1,
    BelowName = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayOverride {
    #[default]
    Inherit,
    Hidden,
    Objective(ObjectiveId),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)] // Public game API also supports arbitrary score entries.
pub enum ScoreHolder {
    Player(i32),
    /// A compile-time sidebar line identifier.
    Line(LineId),
    /// A stable key (1–64 bytes) unrelated to the displayed text.
    Entry(String),
}

impl From<crate::text::InvalidText> for ScoreboardError {
    fn from(_: crate::text::InvalidText) -> Self {
        Self::InvalidText
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScoreboardError {
    #[error("unknown or foreign scoreboard objective")]
    Objective,
    #[error("player is no longer in this instance")]
    Player,
    #[error("personal objective belongs to a different viewer")]
    Audience,
    #[error("scoreboard capacity exceeded")]
    Capacity,
    #[error("invalid or oversized scoreboard text")]
    InvalidText,
    #[error("custom score key must contain 1–64 bytes")]
    InvalidKey,
    #[error("sidebar line must name an existing position from 0 to 14")]
    InvalidLine,
    #[error("static sidebar layout cannot be modified; publish a line update instead")]
    ImmutableLayout,
    #[error("static sidebar layouts can only occupy the sidebar slot")]
    SidebarSlot,
    #[error("could not encode scoreboard data")]
    Encode,
}

fn encode<'p>(packet: &impl Payload<'p>) -> Result<Rc<[u8]>, ScoreboardError> {
    let mut bytes = Vec::new();
    block_on(packet.write_packet(&mut bytes)).map_err(|_| ScoreboardError::Encode)?;
    Ok(Rc::from(bytes))
}

fn objective_name(serial: u64) -> String {
    format!("halcyon_{serial:x}")
}

struct Entry {
    holder: Rc<str>,
    frame: Rc<[u8]>,
    revision: u64,
}

struct Objective {
    owner: Option<i32>,
    name: Rc<str>,
    base: Option<Rc<SidebarBase>>,
    create: Rc<[u8]>,
    update: Rc<[u8]>,
    properties_revision: u64,
    revision: u64,
    scores: BTreeMap<ScoreHolder, Entry>,
}

impl Objective {
    fn score(&self, key: &ScoreHolder) -> Option<&Entry> {
        self.scores.get(key).or_else(|| self.base.as_ref()?.scores.get(key))
    }

    fn score_count(&self) -> usize {
        self.base.as_ref().map_or(self.scores.len(), |base| base.scores.len())
    }

    fn next_score(&self, after: Option<&ScoreHolder>) -> Option<(&ScoreHolder, &Entry)> {
        use std::ops::Bound::{Excluded, Unbounded};
        let start = after.map_or(Unbounded, Excluded);
        let update = self.scores.range::<ScoreHolder, _>((start, Unbounded)).next();
        let base = self
            .base
            .as_ref()
            .and_then(|base| base.scores.range::<ScoreHolder, _>((start, Unbounded)).next());
        match (update, base) {
            (Some(update), Some(base)) => Some(if update.0 <= base.0 { update } else { base }),
            (update, base) => update.or(base),
        }
    }
}

mod template;
use template::SidebarBase;
pub use template::{LineId, SidebarLayout, SidebarLine};

#[derive(Default)]
pub struct Board {
    instance: Option<InstanceId>,
    next: u64,
    revision: u64,
    objectives: BTreeMap<u64, Objective>,
    slots: [Option<u64>; 3],
    viewers: HashMap<i32, [DisplayOverride; 3]>,
    score_count: usize,
    bases: HashMap<&'static SidebarLayout, Rc<SidebarBase>>,
}

impl Board {
    pub(crate) fn bind(&mut self, instance: InstanceId) {
        debug_assert!(self.instance.is_none_or(|id| id == instance));
        self.instance = Some(instance);
    }

    fn serial(&self, id: ObjectiveId) -> Result<u64, ScoreboardError> {
        if self.instance != Some(id.instance) || !self.objectives.contains_key(&id.serial) {
            return Err(ScoreboardError::Objective);
        }
        Ok(id.serial)
    }

    pub(crate) fn create(
        &mut self,
        owner: Option<i32>,
        spec: ObjectiveSpec,
    ) -> Result<ObjectiveId, ScoreboardError> {
        spec.validate()?;
        if self.objectives.len() == MAX_OBJECTIVES {
            return Err(ScoreboardError::Capacity);
        }
        let serial = self.next.checked_add(1).ok_or(ScoreboardError::Capacity)?;
        let name = objective_name(serial);
        let create =
            encode(&SetObjective { name: &name, action: ObjectiveAction::Create(spec.wire()) })?;
        let update =
            encode(&SetObjective { name: &name, action: ObjectiveAction::Update(spec.wire()) })?;
        self.next = serial;
        self.revision += 1;
        self.objectives.insert(
            serial,
            Objective {
                owner,
                name: Rc::from(name),
                base: None,
                create,
                update,
                properties_revision: self.revision,
                revision: self.revision,
                scores: BTreeMap::new(),
            },
        );
        Ok(ObjectiveId { instance: self.instance.expect("GameCtx binds scoreboard"), serial })
    }

    pub(crate) fn update(
        &mut self,
        id: ObjectiveId,
        spec: ObjectiveSpec,
    ) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        if self.objectives[&serial].base.is_some() {
            return Err(ScoreboardError::ImmutableLayout);
        }
        spec.validate()?;
        let name = objective_name(serial);
        let create =
            encode(&SetObjective { name: &name, action: ObjectiveAction::Create(spec.wire()) })?;
        let update =
            encode(&SetObjective { name: &name, action: ObjectiveAction::Update(spec.wire()) })?;
        self.revision += 1;
        let objective = self.objectives.get_mut(&serial).unwrap();
        objective.create = create;
        objective.update = update;
        objective.properties_revision = self.revision;
        objective.revision = self.revision;
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: ObjectiveId) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        self.score_count -= self.objectives.remove(&serial).unwrap().score_count();
        for slot in &mut self.slots {
            if *slot == Some(serial) {
                *slot = None;
            }
        }
        for slots in self.viewers.values_mut() {
            for slot in slots {
                if *slot == DisplayOverride::Objective(id) {
                    *slot = DisplayOverride::Hidden;
                }
            }
        }
        self.revision += 1;
        Ok(())
    }

    fn audience(&self, serial: u64, viewer: Option<i32>) -> Result<(), ScoreboardError> {
        if self.objectives[&serial].owner.is_some_and(|owner| Some(owner) != viewer) {
            return Err(ScoreboardError::Audience);
        }
        Ok(())
    }

    pub(crate) fn display(
        &mut self,
        slot: DisplaySlot,
        id: Option<ObjectiveId>,
    ) -> Result<(), ScoreboardError> {
        let serial = id.map(|id| self.serial(id)).transpose()?;
        if let Some(serial) = serial {
            self.audience(serial, None)?;
            self.validate_slot(serial, slot)?;
        }
        if self.slots[slot as usize] != serial {
            self.slots[slot as usize] = serial;
            self.revision += 1;
        }
        Ok(())
    }

    pub(crate) fn display_for(
        &mut self,
        viewer: i32,
        slot: DisplaySlot,
        value: DisplayOverride,
    ) -> Result<(), ScoreboardError> {
        if let DisplayOverride::Objective(id) = value {
            let serial = self.serial(id)?;
            self.audience(serial, Some(viewer))?;
            self.validate_slot(serial, slot)?;
        }
        let current = self
            .viewers
            .get(&viewer)
            .map_or(DisplayOverride::Inherit, |slots| slots[slot as usize]);
        if current == value {
            return Ok(());
        }
        let slots = self.viewers.entry(viewer).or_default();
        slots[slot as usize] = value;
        if *slots == [DisplayOverride::Inherit; 3] {
            self.viewers.remove(&viewer);
        }
        self.revision += 1;
        Ok(())
    }

    fn selected(&self, viewer: i32) -> [Option<u64>; 3] {
        std::array::from_fn(|slot| match self.viewers.get(&viewer).map(|slots| slots[slot]) {
            Some(DisplayOverride::Hidden) => None,
            Some(DisplayOverride::Objective(id)) => Some(id.serial),
            _ => self.slots[slot],
        })
    }

    fn validate_slot(&self, serial: u64, slot: DisplaySlot) -> Result<(), ScoreboardError> {
        if self.objectives[&serial].base.is_some() && slot != DisplaySlot::Sidebar {
            return Err(ScoreboardError::SidebarSlot);
        }
        Ok(())
    }

    fn entry(
        &self,
        id: ObjectiveId,
        key: ScoreHolder,
        holder: Rc<str>,
        score: Score,
    ) -> Result<Entry, ScoreboardError> {
        self.serial(id)?;
        score.validate()?;
        if let ScoreHolder::Entry(key) = &key
            && (key.is_empty() || key.len() > 64)
        {
            return Err(ScoreboardError::InvalidKey);
        }
        let name = objective_name(id.serial);
        let frame = encode(&SetScore {
            holder: &holder,
            objective: &name,
            value: score.value,
            display: score.display.as_ref().map(Text::nbt),
            number_format: score.number_format.as_ref().map(NumberFormat::wire),
        })?;
        Ok(Entry { holder, frame, revision: self.revision + 1 })
    }

    pub(crate) fn set_score(
        &mut self,
        id: ObjectiveId,
        key: ScoreHolder,
        holder: Rc<str>,
        score: Score,
    ) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        if self.objectives[&serial].base.is_some() {
            return Err(ScoreboardError::ImmutableLayout);
        }
        if matches!(key, ScoreHolder::Line(_)) {
            return Err(ScoreboardError::InvalidKey);
        }
        let old = self.objectives[&serial].scores.get(&key);
        if old.is_none() && self.score_count == MAX_SCORES {
            return Err(ScoreboardError::Capacity);
        }
        let entry = self.entry(id, key.clone(), holder, score)?;
        self.revision += 1;
        let objective = self.objectives.get_mut(&serial).unwrap();
        if objective.scores.insert(key, entry).is_none() {
            self.score_count += 1;
        }
        objective.revision = self.revision;
        Ok(())
    }

    pub(crate) fn reset_score(
        &mut self,
        id: ObjectiveId,
        key: &ScoreHolder,
    ) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        if self.objectives[&serial].base.is_some() {
            return Err(ScoreboardError::ImmutableLayout);
        }
        let objective = self.objectives.get_mut(&serial).unwrap();
        if objective.scores.remove(key).is_some() {
            self.score_count -= 1;
            self.revision += 1;
            objective.revision = self.revision;
        }
        Ok(())
    }

    pub(crate) fn player_left(&mut self, player: i32) {
        let owned: Vec<_> = self
            .objectives
            .iter()
            .filter(|(_, o)| o.owner == Some(player))
            .map(|(&serial, _)| ObjectiveId { instance: self.instance.unwrap(), serial })
            .collect();
        for id in owned {
            let _ = self.remove(id);
        }
        if self.viewers.remove(&player).is_some() {
            self.revision += 1;
        }
        for objective in self.objectives.values_mut() {
            if objective.scores.remove(&ScoreHolder::Player(player)).is_some() {
                self.score_count -= 1;
                self.revision += 1;
                objective.revision = self.revision;
            }
        }
    }

    /// Retained payload estimate, excluding B-tree nodes, Rc headers and allocator overhead.
    #[cfg(test)]
    pub(crate) fn retained_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self.viewers.len() * size_of::<(i32, [DisplayOverride; 3])>()
            + self.bases.values().map(|base| base.retained_payload_bytes()).sum::<usize>()
            + self
                .objectives
                .values()
                .map(|objective| {
                    size_of::<(u64, Objective)>()
                        + if objective.base.is_none() {
                            objective.name.len() + objective.create.len() + objective.update.len()
                        } else {
                            0
                        }
                        + objective
                            .scores
                            .iter()
                            .map(|(key, entry)| {
                                size_of::<(ScoreHolder, Entry)>()
                                    + entry.holder.len()
                                    + entry.frame.len()
                                    + match key {
                                        ScoreHolder::Entry(key) => key.len(),
                                        _ => 0,
                                    }
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

#[cfg(test)]
#[path = "../tests/tests_scoreboard.rs"]
mod tests;
