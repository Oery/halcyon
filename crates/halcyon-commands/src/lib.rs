//! Typed command syntax, independent of transport and gameplay execution.
//!
//! ```
//! use halcyon_commands::{CommandArgs, CommandSet, CommandValue, PlayerName};
//!
//! #[derive(Debug, CommandValue)]
//! enum Mode {
//!     #[value(alias = "0")]
//!     Survival,
//!     #[value(alias = "1")]
//!     Creative,
//! }
//!
//! #[derive(Debug, CommandArgs)]
//! struct Gamemode<'p> {
//!     mode: Mode,
//!     #[arg(name = "player")]
//!     target: Option<PlayerName<'p>>,
//! }
//!
//! #[derive(Debug, CommandSet)]
//! enum Command<'p> {
//!     #[command(name = "gamemode")]
//!     GameMode(Gamemode<'p>),
//! }
//!
//! let Command::GameMode(args) = Command::parse("gamemode 1 Alex").unwrap();
//! assert!(matches!(args.mode, Mode::Creative));
//! assert_eq!(args.target.unwrap().as_str(), "Alex");
//! assert_eq!(Command::complete("/gamemode cr", &()).matches, ["creative"]);
//! ```
//!
//! Struct fields are positional; `Option<T>` fields must trail required fields.
//! An enum deriving `CommandArgs` provides alternative forms: each tuple variant
//! holds one argument struct, and the first complete parse wins. Their command
//! trees share common prefixes, and completion combines the matching forms.
//! [`Position`] consumes three world coordinates, including `~` offsets.
//! The derives support nongeneric types and types with one borrowing lifetime.
//! Command/value names default to lowercase identifiers; `name = "..."` overrides
//! them, and repeated `#[value(alias = "...")]` attributes add enum spellings.
//! An enum-level `#[value(into = path::Enum)]` also generates `From<Self>` for
//! the target enum by matching Rust variant names, regardless of CLI names or aliases.
//! Implement [`CommandArgument`] for local argument types to add syntax and
//! completion providers. Execution and permission checks remain ordinary Rust.

extern crate self as halcyon_commands;

use std::fmt;
use std::str::SplitWhitespace;

pub use halcyon_commands_macros::{CommandArgs, CommandSet, CommandValue};

mod argument;
mod tree;
pub use argument::PlayerName;
mod position;
pub use position::{Coordinate, Position};
pub use tree::{CommandTree, Node, NodeKind};

/// A finite choice becomes literal branches; other kinds become typed arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentKind {
    Choices(&'static [&'static str]),
    Word { suggestions: Option<&'static str> },
    Integer,
    Long,
    Float,
    Double,
    Position,
}

impl ArgumentKind {
    fn suggest(self, prefix: &str, context: &dyn CompletionContext) -> Vec<String> {
        match self {
            Self::Choices(values) => values
                .iter()
                .filter(|value| value.starts_with(prefix))
                .map(|value| (*value).to_owned())
                .collect(),
            Self::Word { suggestions: Some(provider) } => context.suggest(provider, prefix),
            _ => Vec::new(),
        }
    }
}

/// Parsing is lexical. Lookups and mutable world validation belong to execution.
pub trait CommandArgument<'p>: Sized {
    const KIND: ArgumentKind;

    fn parse(value: &'p str) -> Result<Self, &'static str>;

    /// Multiword arguments may consume additional tokens after their first word.
    fn parse_remaining(
        first: &'p str,
        _words: &mut SplitWhitespace<'p>,
    ) -> Result<Self, &'static str> {
        Self::parse(first)
    }

    /// Validate an earlier argument, or return completion for its unfinished tail.
    fn suggest_remaining(
        first: &'p str,
        words: &mut SplitWhitespace<'p>,
        _prefix: &str,
        _context: &dyn CompletionContext,
    ) -> Result<(), Vec<String>> {
        Self::parse_remaining(first, words).map(|_| ()).map_err(|_| Vec::new())
    }

    fn suggest(prefix: &str, context: &dyn CompletionContext) -> Vec<String> {
        Self::KIND.suggest(prefix, context)
    }
}

/// A borrowed application context; no handler allocation or global registry.
pub trait CompletionContext {
    fn suggest(&self, provider: &str, prefix: &str) -> Vec<String>;
}

impl CompletionContext for () {
    fn suggest(&self, _provider: &str, _prefix: &str) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgumentSpec {
    pub name: &'static str,
    pub optional: bool,
    pub kind: ArgumentKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub syntax: ArgumentSyntax,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentSyntax {
    Sequence(&'static [ArgumentSpec]),
    Alternatives(&'static [ArgumentSyntax]),
}

impl ArgumentSyntax {
    pub(crate) fn sequences(self, visit: &mut impl FnMut(&'static [ArgumentSpec])) {
        match self {
            Self::Sequence(arguments) => visit(arguments),
            Self::Alternatives(forms) => {
                for form in forms {
                    form.sequences(visit);
                }
            }
        }
    }
}

impl CommandSpec {
    pub fn usage(self) -> String {
        let mut forms = Vec::new();
        self.syntax.sequences(&mut |arguments| {
            let mut usage = format!("/{}", self.name);
            for argument in arguments {
                usage.push(' ');
                usage.push(if argument.optional { '[' } else { '<' });
                match argument.kind {
                    ArgumentKind::Choices(values) => usage.push_str(&values.join("|")),
                    ArgumentKind::Position => usage.push_str("x y z"),
                    _ => usage.push_str(argument.name),
                }
                usage.push(if argument.optional { ']' } else { '>' });
            }
            forms.push(usage);
        });
        forms.join(" | ")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArgumentError {
    Missing(&'static str),
    Invalid { name: &'static str, message: &'static str },
    Extra,
}

impl fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "Missing argument '{name}'."),
            Self::Invalid { name, message } => write!(f, "Invalid argument '{name}': {message}"),
            Self::Extra => f.write_str("Too many arguments."),
        }
    }
}

impl std::error::Error for ArgumentError {}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    UnknownCommand,
    Arguments { command: CommandSpec, error: ArgumentError },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand => f.write_str("Unknown command."),
            Self::Arguments { command, error } => write!(f, "{error} Usage: {}", command.usage()),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnknownCommand => None,
            Self::Arguments { error, .. } => Some(error),
        }
    }
}

pub trait CommandArgs<'p>: Sized {
    const SYNTAX: ArgumentSyntax;

    fn parse(words: &mut SplitWhitespace<'p>) -> Result<Self, ArgumentError>;

    fn suggest(
        words: SplitWhitespace<'p>,
        prefix: &str,
        context: &dyn CompletionContext,
    ) -> Vec<String>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    /// Java/Minecraft string indices count UTF-16 units, not UTF-8 bytes.
    pub start: i32,
    pub length: i32,
    pub matches: Vec<String>,
}

pub trait CommandSet<'p>: Sized {
    const COMMANDS: &'static [CommandSpec];

    /// Command text excludes the leading slash, as on the Minecraft wire.
    fn parse(text: &'p str) -> Result<Self, ParseError>;

    fn suggest(
        command: &str,
        words: SplitWhitespace<'p>,
        prefix: &str,
        context: &dyn CompletionContext,
    ) -> Vec<String>;

    fn tree() -> CommandTree {
        CommandTree::new(Self::COMMANDS)
    }

    fn complete(text: &'p str, context: &dyn CompletionContext) -> Completion {
        let input = text.strip_prefix('/').unwrap_or(text);
        let offset = text.len() - input.len();
        let split = input
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        let prefix = &input[split..];
        let mut words = input[..split].split_whitespace();
        let mut matches = match words.next() {
            None => Self::COMMANDS
                .iter()
                .map(|command| command.name)
                .filter(|name| name.starts_with(prefix))
                .map(str::to_owned)
                .collect(),
            Some(command) => Self::suggest(command, words, prefix, context),
        };
        matches.sort_unstable();
        matches.dedup();
        matches.truncate(64);
        Completion {
            start: text[..offset + split].encode_utf16().count().try_into().unwrap_or(i32::MAX),
            length: prefix.encode_utf16().count().try_into().unwrap_or(i32::MAX),
            matches,
        }
    }
}

#[cfg(test)]
#[path = "tests/tests_commands.rs"]
mod tests;
