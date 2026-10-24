//! Immutable sidebar definitions and event-driven line publication.
use std::fmt::{self, Write};

use mc_protocol::packets::client::ScoreLine;

use super::*;

/// Stable position in a compile-time layout; construction checks the client limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineId(usize);

impl LineId {
    pub const fn new(index: usize) -> Self {
        assert!(index < MAX_SIDEBAR_LINES, "sidebar position must be below 15");
        Self(index)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SidebarLine {
    pub text: &'static str,
    pub style: Style,
}

impl SidebarLine {
    pub const fn new(text: &'static str) -> Self {
        Self { text, style: Style::PLAIN }
    }
}

/// Declare as static Rust data. The title, initial labels, ordering and styles
/// never change. Runtime values are published separately with `update_line`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct SidebarLayout {
    pub title: &'static str,
    pub title_style: Style,
    pub lines: &'static [SidebarLine],
}

impl SidebarLayout {
    pub const fn new(title: &'static str, lines: &'static [SidebarLine]) -> Self {
        assert!(lines.len() <= MAX_SIDEBAR_LINES, "sidebar has more than 15 lines");
        Self { title, title_style: Style::PLAIN, lines }
    }
}

pub(super) struct SidebarBase {
    pub(super) layout: &'static SidebarLayout,
    pub(super) name: Rc<str>,
    pub(super) create: Rc<[u8]>,
    pub(super) scores: BTreeMap<ScoreHolder, Entry>,
}

const HOLDERS: [&str; MAX_SIDEBAR_LINES] = [
    "$halcyon:line_0",
    "$halcyon:line_1",
    "$halcyon:line_2",
    "$halcyon:line_3",
    "$halcyon:line_4",
    "$halcyon:line_5",
    "$halcyon:line_6",
    "$halcyon:line_7",
    "$halcyon:line_8",
    "$halcyon:line_9",
    "$halcyon:line_10",
    "$halcyon:line_11",
    "$halcyon:line_12",
    "$halcyon:line_13",
    "$halcyon:line_14",
];

fn line_frame(
    name: &str,
    index: usize,
    style: Style,
    text: &str,
) -> Result<Rc<[u8]>, ScoreboardError> {
    let decorations = u8::from(style.bold)
        | (u8::from(style.italic) << 1)
        | (u8::from(style.underlined) << 2)
        | (u8::from(style.strikethrough) << 3)
        | (u8::from(style.obfuscated) << 4);
    let packet = ScoreLine {
        holder: HOLDERS[index],
        objective: name,
        value: (MAX_SIDEBAR_LINES - index) as i32,
        text,
        color: style.color,
        decorations,
    };
    if packet.component_len() > MAX_TEXT_BYTES {
        return Err(ScoreboardError::InvalidText);
    }
    // Both formatted text and encoding scratch live on the stack. The only
    // payload allocation is the immutable frame retained for delivery.
    let mut frame = [0u8; MAX_TEXT_BYTES + 256];
    let len = packet.encode_into(&mut frame).map_err(|_| ScoreboardError::Encode)?;
    Ok(Rc::from(&frame[..len]))
}

impl SidebarBase {
    fn new(layout: &'static SidebarLayout, serial: u64) -> Result<Self, ScoreboardError> {
        layout.title_style.validate()?;
        if layout.title.len() > MAX_TEXT_BYTES {
            return Err(ScoreboardError::InvalidText);
        }
        let Nbt::Compound(mut fields) = layout.title_style.nbt() else {
            unreachable!()
        };
        fields.push(("text", Nbt::Str(layout.title)));
        let title = Nbt::Compound(fields);
        if title.body_len() > MAX_TEXT_BYTES {
            return Err(ScoreboardError::InvalidText);
        }
        let name: Rc<str> = Rc::from(format!("halcyon_sidebar_{serial:x}"));
        let create = encode(&SetObjective {
            name: &name,
            action: ObjectiveAction::Create(ObjectiveProperties {
                title,
                render: ScoreRender::Integer,
                number_format: Some(ScoreNumberFormat::Blank),
            }),
        })?;
        let mut scores = BTreeMap::new();
        for (index, line) in layout.lines.iter().enumerate() {
            line.style.validate()?;
            let frame = line_frame(&name, index, line.style, line.text)?;
            scores.insert(
                ScoreHolder::Line(LineId(index)),
                Entry { holder: Rc::from(HOLDERS[index]), frame, revision: 0 },
            );
        }
        Ok(Self { layout, name, create, scores })
    }

    #[cfg(test)]
    pub(super) fn retained_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self.name.len()
            + self.create.len()
            + self
                .scores
                .values()
                .map(|entry| {
                    size_of::<(ScoreHolder, Entry)>() + entry.holder.len() + entry.frame.len()
                })
                .sum::<usize>()
    }
}

struct LineBuffer {
    bytes: [u8; MAX_TEXT_BYTES],
    len: usize,
}

impl Write for LineBuffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self
            .len
            .checked_add(text.len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or(fmt::Error)?;
        self.bytes[self.len..end].copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl Board {
    pub(crate) fn create_sidebar(
        &mut self,
        owner: Option<i32>,
        layout: &'static SidebarLayout,
    ) -> Result<ObjectiveId, ScoreboardError> {
        if self.objectives.len() == MAX_OBJECTIVES
            || layout.lines.len() > MAX_SIDEBAR_LINES
            || self.score_count + layout.lines.len() > MAX_SCORES
        {
            return Err(ScoreboardError::Capacity);
        }
        let serial = self.next.checked_add(1).ok_or(ScoreboardError::Capacity)?;
        let base = if let Some(base) = self.bases.get(layout) {
            base.clone()
        } else {
            if self.bases.len() == MAX_OBJECTIVES {
                self.bases.retain(|_, base| Rc::strong_count(base) > 1);
                if self.bases.len() == MAX_OBJECTIVES {
                    return Err(ScoreboardError::Capacity);
                }
            }
            let base = Rc::new(SidebarBase::new(layout, serial)?);
            self.bases.insert(layout, base.clone());
            base
        };
        self.next = serial;
        self.revision += 1;
        self.score_count += layout.lines.len();
        self.objectives.insert(
            serial,
            Objective {
                owner,
                name: base.name.clone(),
                create: base.create.clone(),
                update: base.create.clone(),
                properties_revision: self.revision,
                revision: self.revision,
                base: Some(base),
                scores: BTreeMap::new(),
            },
        );
        Ok(ObjectiveId { instance: self.instance.expect("GameCtx binds scoreboard"), serial })
    }

    /// Publish an event's new value unconditionally. The game owns its values;
    /// delivery retains only the latest encoded packet and revision per line.
    pub(crate) fn update_line(
        &mut self,
        id: ObjectiveId,
        line: LineId,
        text: fmt::Arguments<'_>,
    ) -> Result<(), ScoreboardError> {
        self.update_lines(std::iter::once(id), line, text)
    }

    /// A shared value can be encoded once for several personal views of the
    /// same static layout. Every target is validated before anything changes.
    pub(crate) fn update_lines(
        &mut self,
        ids: impl Iterator<Item = ObjectiveId> + Clone,
        line: LineId,
        text: fmt::Arguments<'_>,
    ) -> Result<(), ScoreboardError> {
        let Some(first) = ids.clone().next() else {
            return Ok(());
        };
        let serial = self.serial(first)?;
        let base = self.objectives[&serial].base.as_ref().ok_or(ScoreboardError::InvalidLine)?;
        let style = base.layout.lines.get(line.0).ok_or(ScoreboardError::InvalidLine)?.style;
        for (index, id) in ids.clone().enumerate() {
            if index == MAX_OBJECTIVES {
                return Err(ScoreboardError::Capacity);
            }
            let objective = &self.objectives[&self.serial(id)?];
            if objective.base.as_ref().is_none_or(|other| !Rc::ptr_eq(base, other)) {
                return Err(ScoreboardError::InvalidLine);
            }
        }
        let mut buffer = LineBuffer { bytes: [0; MAX_TEXT_BYTES], len: 0 };
        fmt::write(&mut buffer, text).map_err(|_| ScoreboardError::InvalidText)?;
        let text = std::str::from_utf8(&buffer.bytes[..buffer.len])
            .map_err(|_| ScoreboardError::InvalidText)?;
        let frame = line_frame(&base.name, line.0, style, text)?;
        let holder = base.scores[&ScoreHolder::Line(line)].holder.clone();
        self.revision += 1;
        for id in ids {
            let objective = self.objectives.get_mut(&id.serial).unwrap();
            objective.scores.insert(
                ScoreHolder::Line(line),
                Entry { holder: holder.clone(), frame: frame.clone(), revision: self.revision },
            );
            objective.revision = self.revision;
        }
        Ok(())
    }
}
