//! Only supported block-entity data survives the cold Anvil import.
use crate::disk_nbt::DiskNbt;

pub const MAX_SIGN_BYTES: usize = 16_384;
pub const MAX_BLOCK_ENTITIES_PER_COLUMN: usize = 128;

#[derive(Clone, Debug)]
pub struct SignFace {
    pub messages: [DiskNbt; 4],
    pub color: String,
    pub glowing: bool,
}

impl Default for SignFace {
    fn default() -> Self {
        Self {
            messages: std::array::from_fn(|_| DiskNbt::Str(String::new())),
            color: "black".to_owned(),
            glowing: false,
        }
    }
}

impl SignFace {
    pub fn plain(lines: [String; 4]) -> Self {
        Self { messages: lines.map(DiskNbt::Str), ..Self::default() }
    }

    fn sanitized(&self, remaining: &mut usize) -> Option<Self> {
        const COLORS: [&str; 16] = [
            "white",
            "orange",
            "magenta",
            "light_blue",
            "yellow",
            "lime",
            "pink",
            "gray",
            "light_gray",
            "cyan",
            "purple",
            "blue",
            "brown",
            "green",
            "red",
            "black",
        ];
        if !COLORS.contains(&self.color.as_str()) {
            return None;
        }
        let messages: Vec<_> = self
            .messages
            .iter()
            .map(|message| {
                let value = text(message, 0, remaining)?;
                Some(match value {
                    DiskNbt::Str(_) => DiskNbt::Compound(vec![("text".to_owned(), value)]),
                    DiskNbt::Compound(_) => value,
                    DiskNbt::List(_, _) => DiskNbt::Compound(vec![
                        ("text".to_owned(), DiskNbt::Str(String::new())),
                        ("extra".to_owned(), value),
                    ]),
                    _ => return None,
                })
            })
            .collect::<Option<_>>()?;
        Some(Self {
            messages: messages.try_into().ok()?,
            color: self.color.clone(),
            glowing: self.glowing,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Sign {
    pub front: SignFace,
    pub back: SignFace,
}

impl Sign {
    pub fn payload_bytes(&self) -> usize {
        fn size(value: &DiskNbt) -> usize {
            match value {
                DiskNbt::Str(s) => s.capacity(),
                DiskNbt::List(_, items) => {
                    items.capacity() * std::mem::size_of::<DiskNbt>()
                        + items.iter().map(size).sum::<usize>()
                }
                DiskNbt::Compound(fields) => {
                    fields.capacity() * std::mem::size_of::<(String, DiskNbt)>()
                        + fields.iter().map(|(k, v)| k.capacity() + size(v)).sum::<usize>()
                }
                _ => 0,
            }
        }
        std::mem::size_of::<Self>()
            + [&self.front, &self.back]
                .iter()
                .map(|f| f.color.capacity() + f.messages.iter().map(size).sum::<usize>())
                .sum::<usize>()
    }
    pub fn sanitized(&self) -> Option<Self> {
        let mut remaining = MAX_SIGN_BYTES;
        Some(Self {
            front: self.front.sanitized(&mut remaining)?,
            back: self.back.sanitized(&mut remaining)?,
        })
    }

    pub fn from_nbt(data: &DiskNbt) -> Option<Self> {
        fn face(data: Option<&DiskNbt>) -> Option<SignFace> {
            let Some(data) = data else {
                return Some(SignFace::default());
            };
            let messages = data.get("messages")?.as_list()?;
            if messages.len() != 4 {
                return None;
            }
            Some(SignFace {
                messages: std::array::from_fn(|i| messages[i].clone()),
                color: data.get("color").and_then(DiskNbt::as_str).unwrap_or("black").to_owned(),
                glowing: data.get("has_glowing_text").and_then(DiskNbt::as_i8).unwrap_or(0) != 0,
            })
        }
        Self { front: face(data.get("front_text"))?, back: face(data.get("back_text"))? }
            .sanitized()
    }
}

/// Retain display components only; executable actions never enter network state.
fn text(value: &DiskNbt, depth: usize, remaining: &mut usize) -> Option<DiskNbt> {
    if depth > 16 {
        return None;
    }
    *remaining = remaining.checked_sub(8)?;
    Some(match value {
        DiskNbt::Str(value) => {
            *remaining = remaining.checked_sub(value.len())?;
            DiskNbt::Str(value.clone())
        }
        DiskNbt::Byte(value) => DiskNbt::Byte(*value),
        DiskNbt::Int(value) => DiskNbt::Int(*value),
        DiskNbt::List(kind, values) if values.len() <= 32 => DiskNbt::List(
            *kind,
            values.iter().map(|v| text(v, depth + 1, remaining)).collect::<Option<_>>()?,
        ),
        DiskNbt::Compound(values) if values.len() <= 32 => {
            let mut fields: Vec<_> = values
                .iter()
                .filter(|(key, _)| {
                    matches!(
                        key.as_str(),
                        "text"
                            | "translate"
                            | "fallback"
                            | "with"
                            | "extra"
                            | "color"
                            | "bold"
                            | "italic"
                            | "underlined"
                            | "strikethrough"
                            | "obfuscated"
                            | "font"
                            | "shadow_color"
                    )
                })
                .map(|(k, v)| Some((k.clone(), text(v, depth + 1, remaining)?)))
                .collect::<Option<_>>()?;
            if !fields.iter().any(|(key, _)| matches!(key.as_str(), "text" | "translate")) {
                fields.push(("text".to_owned(), DiskNbt::Str(String::new())));
            }
            DiskNbt::Compound(fields)
        }
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub enum BlockEntityData {
    Sign(std::sync::Arc<Sign>),
    Jukebox { disc: Option<String> },
}

#[derive(Clone, Debug)]
pub struct BlockEntity {
    pub pos: [i32; 3],
    pub data: BlockEntityData,
}
