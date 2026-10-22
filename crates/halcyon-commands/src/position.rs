use std::str::SplitWhitespace;

use super::{ArgumentKind, CommandArgument, CompletionContext};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coordinate {
    Absolute(f64),
    Relative(f64),
}

impl Coordinate {
    fn parse(value: &str, center: bool) -> Result<Self, &'static str> {
        if let Some(offset) = value.strip_prefix('~') {
            let offset = if offset.is_empty() {
                0.0
            } else {
                <f64 as CommandArgument>::parse(offset)?
            };
            Ok(Self::Relative(offset))
        } else {
            let number = <f64 as CommandArgument>::parse(value)?;
            Ok(Self::Absolute(number + if center && !value.contains('.') { 0.5 } else { 0.0 }))
        }
    }

    fn resolve(self, origin: f64) -> f64 {
        match self {
            Self::Absolute(value) => value,
            Self::Relative(offset) => origin + offset,
        }
    }
}

/// Minecraft world coordinates. Bare integer X/Z values address block centers;
/// relative coordinates use the supplied origin. Local ^ axes are excluded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position(pub [Coordinate; 3]);

impl Position {
    pub fn resolve(self, origin: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| self.0[axis].resolve(origin[axis]))
    }

    fn from_words(values: [&str; 3]) -> Result<Self, &'static str> {
        Ok(Self([
            Coordinate::parse(values[0], true)?,
            Coordinate::parse(values[1], false)?,
            Coordinate::parse(values[2], true)?,
        ]))
    }
}

impl<'p> CommandArgument<'p> for Position {
    const KIND: ArgumentKind = ArgumentKind::Position;

    fn parse(value: &'p str) -> Result<Self, &'static str> {
        let mut words = value.split_whitespace();
        let first = words.next().ok_or("Expected three world coordinates.")?;
        let position = Self::parse_remaining(first, &mut words)?;
        if words.next().is_some() {
            return Err("Expected three world coordinates.");
        }
        Ok(position)
    }

    fn parse_remaining(
        first: &'p str,
        words: &mut SplitWhitespace<'p>,
    ) -> Result<Self, &'static str> {
        Self::from_words([
            first,
            words.next().ok_or("Expected three world coordinates.")?,
            words.next().ok_or("Expected three world coordinates.")?,
        ])
    }

    fn suggest(prefix: &str, _context: &dyn CompletionContext) -> Vec<String> {
        if "~".starts_with(prefix) {
            vec!["~".to_owned()]
        } else {
            Vec::new()
        }
    }

    fn suggest_remaining(
        first: &'p str,
        words: &mut SplitWhitespace<'p>,
        prefix: &str,
        context: &dyn CompletionContext,
    ) -> Result<(), Vec<String>> {
        Coordinate::parse(first, true).map_err(|_| Vec::new())?;
        for axis in 1..3 {
            let Some(value) = words.next() else {
                return Err(Self::suggest(prefix, context));
            };
            Coordinate::parse(value, axis == 2).map_err(|_| Vec::new())?;
        }
        Ok(())
    }
}
