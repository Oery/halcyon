//! Command declarations and execution; syntax is derived by halcyon-commands.
use halcyon_commands::{ArgumentKind, CommandSet, Completion, CompletionContext, NodeKind};
use mc_protocol::packets::client::{
    CommandMatch, CommandNode, CommandNodeKind, CommandNumberKind, CommandSuggestionsResponse,
    Commands, SystemChatMessage,
};
use mc_protocol::types::{Nbt, VarInt};

use crate::Client;
use crate::error::Error;
use crate::lobby::{Lobby, Registry};

mod gamemode;
mod give;
mod tp;

#[derive(Debug, thiserror::Error)]
enum CommandError {
    #[error("{0}")]
    Message(&'static str),
    #[error(transparent)]
    Parse(#[from] halcyon_commands::ParseError),
    #[error("Wait until you have spawned before using commands.")]
    NotReady,
    #[error(transparent)]
    PlayerAction(#[from] crate::player::PlayerActionError),
}

#[derive(Debug, CommandSet)]
enum Command<'p> {
    #[command(name = "gamemode")]
    GameMode(gamemode::GameModeCommand<'p>),
    #[command(name = "tp")]
    Teleport(tp::TpCommand<'p>),
    #[command(name = "give")]
    Give(give::GiveCommand<'p>),
}

impl Command<'_> {
    async fn execute(self, lobby: &Lobby, sender: i32) -> Result<String, CommandError> {
        match self {
            Self::GameMode(command) => command.execute(lobby, sender).await,
            Self::Teleport(command) => command.execute(lobby, sender).await,
            Self::Give(command) => command.execute(lobby, sender).await,
        }
    }
}

pub async fn execute(text: &str, client: &mut Client) -> Result<(), Error> {
    let result = async {
        let session = client
            .session
            .as_ref()
            .filter(|session| session.ready())
            .ok_or(CommandError::NotReady)?;
        Command::parse(text)?.execute(&client.lobby, session.entity_id()).await
    }
    .await;
    let failed = result.is_err();
    let message = result.unwrap_or_else(|error| error.to_string());
    client
        .send(&SystemChatMessage {
            content: Nbt::Compound(vec![
                ("text", Nbt::Str(&message)),
                ("color", Nbt::Str(if failed { "red" } else { "white" })),
            ]),
            overlay: false,
        })
        .await
}

/// Adapt the derived, transport-neutral schema once into cached Minecraft frames.
pub fn tree() -> Commands<'static> {
    let tree = Command::tree();
    let nodes = tree
        .nodes
        .into_iter()
        .map(|node| {
            let kind = match node.kind {
                NodeKind::Root => CommandNodeKind::Root,
                NodeKind::Literal(name) => CommandNodeKind::Literal(name),
                NodeKind::Argument(argument) => match argument.kind {
                    ArgumentKind::Word { suggestions } => CommandNodeKind::Word {
                        name: argument.name,
                        suggest: suggestions.is_some(),
                    },
                    ArgumentKind::Integer => CommandNodeKind::Number {
                        name: argument.name,
                        kind: CommandNumberKind::Integer,
                    },
                    ArgumentKind::Long => CommandNodeKind::Number {
                        name: argument.name,
                        kind: CommandNumberKind::Long,
                    },
                    ArgumentKind::Float => CommandNodeKind::Number {
                        name: argument.name,
                        kind: CommandNumberKind::Float,
                    },
                    ArgumentKind::Double => CommandNodeKind::Number {
                        name: argument.name,
                        kind: CommandNumberKind::Double,
                    },
                    ArgumentKind::Position => CommandNodeKind::Position { name: argument.name },
                    ArgumentKind::Choices(_) => unreachable!("choices become literal nodes"),
                },
            };
            CommandNode {
                kind,
                children: node.children.into_iter().map(|id| VarInt(id as i32)).collect(),
                executable: node.executable,
            }
        })
        .collect();
    Commands { nodes, root_index: tree.root as i32 }
}

fn complete(text: &str, registry: &Registry) -> Completion {
    Command::complete(text, registry)
}

impl CompletionContext for Registry {
    fn suggest(&self, provider: &str, prefix: &str) -> Vec<String> {
        match provider {
            "players" => self.command_player_names(prefix),
            "items" => give::suggestions(prefix),
            _ => Vec::new(),
        }
    }
}

pub async fn suggest(text: &str, transaction_id: i32, client: &mut Client) -> Result<(), Error> {
    let completion = client.lobby.with_registry(|registry| complete(text, registry));
    client
        .send(&CommandSuggestionsResponse {
            transaction_id,
            start: completion.start,
            length: completion.length,
            matches: completion.matches.iter().map(|text| CommandMatch { text }).collect(),
        })
        .await
}

#[cfg(test)]
#[path = "../tests/tests_commands.rs"]
mod tests;
