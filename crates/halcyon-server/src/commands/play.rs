use halcyon_commands::{ArgumentKind, CommandArgs, CommandArgument};

use super::Reply;
use crate::game::{Destination, KINDS};

#[derive(Debug)]
pub(super) struct GameKind(&'static str);

impl<'p> CommandArgument<'p> for GameKind {
    const KIND: ArgumentKind = ArgumentKind::Choices(&KINDS);
    fn parse(value: &'p str) -> Result<Self, &'static str> {
        crate::game::kind(value).map(Self).ok_or("Unknown game.")
    }
}

#[derive(Debug, CommandArgs)]
pub(super) struct PlayCommand {
    game: GameKind,
}

impl PlayCommand {
    pub(super) fn execute(self) -> Reply {
        Reply {
            message: format!("Joining {}...", self.game.0),
            destination: Some(Destination::Matchmake(self.game.0)),
        }
    }
}
