use super::{ArgumentKind, CommandArgument};

fn word(value: &str) -> Result<(), &'static str> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+'))
    {
        Ok(())
    } else {
        Err("Expected one unquoted word.")
    }
}

impl<'p> CommandArgument<'p> for &'p str {
    const KIND: ArgumentKind = ArgumentKind::Word { suggestions: None };

    fn parse(value: &'p str) -> Result<Self, &'static str> {
        word(value)?;
        Ok(value)
    }
}

impl<'p> CommandArgument<'p> for String {
    const KIND: ArgumentKind = ArgumentKind::Word { suggestions: None };

    fn parse(value: &'p str) -> Result<Self, &'static str> {
        word(value)?;
        Ok(value.to_owned())
    }
}

/// A validated Minecraft username; resolution remains an application operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerName<'p>(&'p str);

impl<'p> PlayerName<'p> {
    pub fn as_str(self) -> &'p str {
        self.0
    }
}

impl<'p> CommandArgument<'p> for PlayerName<'p> {
    const KIND: ArgumentKind = ArgumentKind::Word { suggestions: Some("players") };

    fn parse(value: &'p str) -> Result<Self, &'static str> {
        if !value.is_empty()
            && value.len() <= 16
            && value.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            Ok(Self(value))
        } else {
            Err("Expected a player name of 1–16 letters, digits or underscores.")
        }
    }
}

impl<'p> CommandArgument<'p> for bool {
    const KIND: ArgumentKind = ArgumentKind::Choices(&["true", "false"]);

    fn parse(value: &'p str) -> Result<Self, &'static str> {
        value.parse().map_err(|_| "Expected true or false.")
    }
}

fn number(value: &str) -> bool {
    // Match Brigadier's numeric syntax; Rust additionally accepts plus/exponents.
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'-' | b'.'))
}

macro_rules! integer {
    ($ty:ty, $kind:ident, $message:literal) => {
        impl<'p> CommandArgument<'p> for $ty {
            const KIND: ArgumentKind = ArgumentKind::$kind;

            fn parse(value: &'p str) -> Result<Self, &'static str> {
                if !number(value) {
                    return Err($message);
                }
                value.parse().map_err(|_| $message)
            }
        }
    };
}

integer!(i32, Integer, "Expected a 32-bit integer.");
integer!(i64, Long, "Expected a 64-bit integer.");

macro_rules! float {
    ($ty:ty, $kind:ident) => {
        impl<'p> CommandArgument<'p> for $ty {
            const KIND: ArgumentKind = ArgumentKind::$kind;

            fn parse(value: &'p str) -> Result<Self, &'static str> {
                if !number(value) {
                    return Err("Expected a finite number.");
                }
                let number: Self = value.parse().map_err(|_| "Expected a finite number.")?;
                if number.is_finite() {
                    Ok(number)
                } else {
                    Err("Expected a finite number.")
                }
            }
        }
    };
}

float!(f32, Float);
float!(f64, Double);
