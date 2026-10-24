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
pub use mc_protocol::packets::client::ScoreRender;

mod delivery;
pub use delivery::ScoreboardObserver;

const MAX_OBJECTIVES: usize = 512;
const MAX_SCORES: usize = 16_384;
const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_SIDEBAR_LINES: usize = 15;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// A vanilla color name or `#rrggbb`.
    pub color: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
}

impl Style {
    fn nbt(&self) -> Nbt<'_> {
        let mut fields = vec![
            ("bold", Nbt::Bool(self.bold)),
            ("italic", Nbt::Bool(self.italic)),
            ("underlined", Nbt::Bool(self.underlined)),
            ("strikethrough", Nbt::Bool(self.strikethrough)),
            ("obfuscated", Nbt::Bool(self.obfuscated)),
        ];
        if let Some(color) = &self.color {
            fields.push(("color", Nbt::Str(color)));
        }
        Nbt::Compound(fields)
    }

    fn validate(&self) -> Result<(), ScoreboardError> {
        if let Some(color) = &self.color {
            const COLORS: &[&str] = &[
                "black",
                "dark_blue",
                "dark_green",
                "dark_aqua",
                "dark_red",
                "dark_purple",
                "gold",
                "gray",
                "dark_gray",
                "blue",
                "green",
                "aqua",
                "red",
                "light_purple",
                "yellow",
                "white",
            ];
            if !COLORS.contains(&color.as_str())
                && !(color.len() == 7
                    && color.starts_with('#')
                    && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit))
            {
                return Err(ScoreboardError::InvalidText);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// Literal text, with explicit styling on each span; no executable text events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text(pub Vec<Span>);

impl From<&str> for Text {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl From<String> for Text {
    fn from(text: String) -> Self {
        Self(vec![Span { text, style: Style::default() }])
    }
}

impl Text {
    fn nbt(&self) -> Nbt<'_> {
        let extra = self
            .0
            .iter()
            .map(|span| {
                let Nbt::Compound(mut fields) = span.style.nbt() else {
                    unreachable!()
                };
                fields.push(("text", Nbt::Str(&span.text)));
                Nbt::Compound(fields)
            })
            .collect();
        Nbt::Compound(vec![("text", Nbt::Str("")), ("extra", Nbt::List(extra))])
    }

    fn validate(&self) -> Result<(), ScoreboardError> {
        // Bound the input before constructing any NBT containers.
        if self.0.len() > 64 || self.0.iter().map(|s| s.text.len()).sum::<usize>() > MAX_TEXT_BYTES
        {
            return Err(ScoreboardError::InvalidText);
        }
        for span in &self.0 {
            span.style.validate()?;
        }
        if self.nbt().body_len() > MAX_TEXT_BYTES {
            return Err(ScoreboardError::InvalidText);
        }
        Ok(())
    }
}

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
            Self::Styled(style) => style.validate(),
            Self::Fixed(text) => text.validate(),
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
pub enum ScoreHolder {
    Player(i32),
    /// A stable key (1–64 bytes) unrelated to the displayed text.
    Entry(String),
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
    score: Score,
    frame: Rc<[u8]>,
    revision: u64,
}

struct Objective {
    owner: Option<i32>,
    spec: ObjectiveSpec,
    create: Rc<[u8]>,
    update: Rc<[u8]>,
    properties_revision: u64,
    revision: u64,
    scores: BTreeMap<ScoreHolder, Entry>,
}

#[derive(Default)]
pub struct Board {
    instance: Option<InstanceId>,
    next: u64,
    revision: u64,
    objectives: BTreeMap<u64, Objective>,
    slots: [Option<u64>; 3],
    viewers: HashMap<i32, [DisplayOverride; 3]>,
    score_count: usize,
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
                spec,
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
        spec.validate()?;
        if self.objectives[&serial].spec == spec {
            return Ok(());
        }
        let name = objective_name(serial);
        let create =
            encode(&SetObjective { name: &name, action: ObjectiveAction::Create(spec.wire()) })?;
        let update =
            encode(&SetObjective { name: &name, action: ObjectiveAction::Update(spec.wire()) })?;
        self.revision += 1;
        let objective = self.objectives.get_mut(&serial).unwrap();
        objective.spec = spec;
        objective.create = create;
        objective.update = update;
        objective.properties_revision = self.revision;
        objective.revision = self.revision;
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: ObjectiveId) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        self.score_count -= self.objectives.remove(&serial).unwrap().scores.len();
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
        Ok(Entry { holder, score, frame, revision: self.revision + 1 })
    }

    pub(crate) fn set_score(
        &mut self,
        id: ObjectiveId,
        key: ScoreHolder,
        holder: Rc<str>,
        score: Score,
    ) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        let old = self.objectives[&serial].scores.get(&key);
        if old.is_some_and(|old| old.score == score && old.holder == holder) {
            return Ok(());
        }
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
        let objective = self.objectives.get_mut(&serial).unwrap();
        if objective.scores.remove(key).is_some() {
            self.score_count -= 1;
            self.revision += 1;
            objective.revision = self.revision;
        }
        Ok(())
    }

    /// The helper owns this objective's scores; validate/encode the whole replacement first.
    pub(crate) fn sidebar(
        &mut self,
        id: ObjectiveId,
        title: Text,
        lines: &[Text],
    ) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        if lines.len() > MAX_SIDEBAR_LINES {
            return Err(ScoreboardError::Capacity);
        }
        let spec =
            ObjectiveSpec { title, number_format: Some(NumberFormat::Blank), ..Default::default() };
        spec.validate()?;
        let old = &self.objectives[&serial];
        if self.score_count - old.scores.len() + lines.len() > MAX_SCORES {
            return Err(ScoreboardError::Capacity);
        }
        let mut entries = BTreeMap::new();
        for (index, text) in lines.iter().enumerate() {
            let key = ScoreHolder::Entry(format!("line_{index}"));
            let score = Score {
                value: (MAX_SIDEBAR_LINES - index) as i32,
                display: Some(text.clone()),
                ..Default::default()
            };
            if let Some(entry) = old.scores.get(&key).filter(|entry| entry.score == score) {
                entries.insert(
                    key,
                    Entry {
                        holder: entry.holder.clone(),
                        score,
                        frame: entry.frame.clone(),
                        revision: entry.revision,
                    },
                );
            } else {
                let holder = Rc::from(format!("$halcyon:line_{index}"));
                entries.insert(key.clone(), self.entry(id, key, holder, score)?);
            }
        }
        let changed = entries.len() != old.scores.len()
            || entries.iter().any(|(key, entry)| {
                old.scores.get(key).is_none_or(|old| old.revision != entry.revision)
            });
        self.update(id, spec)?;
        if changed {
            self.revision += 1;
            let objective = self.objectives.get_mut(&serial).unwrap();
            self.score_count = self.score_count - objective.scores.len() + entries.len();
            objective.scores = entries;
            objective.revision = self.revision;
        }
        Ok(())
    }

    /// Change one initialized sidebar label, preserving its order and format.
    /// Unchanged text returns before encoding; other lines are never rebuilt.
    pub(crate) fn set_sidebar_line(
        &mut self,
        id: ObjectiveId,
        line: usize,
        text: Text,
    ) -> Result<(), ScoreboardError> {
        let serial = self.serial(id)?;
        if line >= MAX_SIDEBAR_LINES {
            return Err(ScoreboardError::InvalidLine);
        }
        let key = ScoreHolder::Entry(format!("line_{line}"));
        let entry =
            self.objectives[&serial].scores.get(&key).ok_or(ScoreboardError::InvalidLine)?;
        let score = Score {
            value: entry.score.value,
            display: Some(text),
            number_format: entry.score.number_format.clone(),
        };
        self.set_score(id, key, entry.holder.clone(), score)
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
        fn text(text: &Text) -> usize {
            text.0.len() * size_of::<Span>()
                + text
                    .0
                    .iter()
                    .map(|span| span.text.len() + span.style.color.as_ref().map_or(0, String::len))
                    .sum::<usize>()
        }
        fn format(format: &Option<NumberFormat>) -> usize {
            match format {
                Some(NumberFormat::Fixed(value)) => text(value),
                Some(NumberFormat::Styled(style)) => style.color.as_ref().map_or(0, String::len),
                _ => 0,
            }
        }
        size_of::<Self>()
            + self.viewers.len() * size_of::<(i32, [DisplayOverride; 3])>()
            + self
                .objectives
                .values()
                .map(|objective| {
                    size_of::<(u64, Objective)>()
                        + text(&objective.spec.title)
                        + format(&objective.spec.number_format)
                        + objective.create.len()
                        + objective.update.len()
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
                                    + entry.score.display.as_ref().map_or(0, text)
                                    + format(&entry.score.number_format)
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

#[cfg(test)]
#[path = "../tests/tests_scoreboard.rs"]
mod tests;
