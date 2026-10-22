use halcyon_commands::{CommandArgs, CommandValue, PlayerName};
use mc_protocol::types::GameMode;

use super::CommandError;
use crate::lobby::Lobby;

#[derive(Debug, CommandValue)]
#[value(into = GameMode)]
pub(super) enum Mode {
    #[value(alias = "0")]
    Survival,
    #[value(alias = "1")]
    Creative,
    #[value(alias = "2")]
    Adventure,
    #[value(alias = "3")]
    Spectator,
}

#[derive(Debug, CommandArgs)]
pub(super) struct GameModeCommand<'p> {
    pub(super) mode: Mode,
    #[arg(name = "player")]
    pub(super) target: Option<PlayerName<'p>>,
}

impl GameModeCommand<'_> {
    pub(super) async fn execute(self, lobby: &Lobby, sender: i32) -> Result<String, CommandError> {
        let mode = self.mode.into();
        let name = lobby.set_game_mode(sender, self.target.map(PlayerName::as_str), mode).await?;
        Ok(format!("Set {name}'s game mode to {mode}."))
    }
}
