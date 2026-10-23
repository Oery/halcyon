use halcyon_commands::{CommandArgs, PlayerName, Position};

use super::CommandError;
use crate::instance::{Instance, TeleportDestination};

#[derive(Debug, CommandArgs)]
pub(super) enum TpCommand<'p> {
    Position(SelfPosition),
    Player(SelfPlayer<'p>),
    TargetPosition(TargetPosition<'p>),
    TargetPlayer(TargetPlayer<'p>),
}

#[derive(Debug, CommandArgs)]
pub(super) struct SelfPosition {
    pub(super) location: Position,
}

#[derive(Debug, CommandArgs)]
pub(super) struct SelfPlayer<'p> {
    pub(super) destination: PlayerName<'p>,
}

#[derive(Debug, CommandArgs)]
pub(super) struct TargetPosition<'p> {
    pub(super) target: PlayerName<'p>,
    pub(super) location: Position,
}

#[derive(Debug, CommandArgs)]
pub(super) struct TargetPlayer<'p> {
    pub(super) target: PlayerName<'p>,
    pub(super) destination: PlayerName<'p>,
}

impl TpCommand<'_> {
    pub(super) async fn execute(
        self,
        instance: &Instance,
        sender: i32,
    ) -> Result<String, CommandError> {
        let (target, destination) = match self {
            Self::Position(args) => (None, TeleportDestination::Position(args.location)),
            Self::Player(args) => (None, TeleportDestination::Player(args.destination.as_str())),
            Self::TargetPosition(args) => {
                (Some(args.target.as_str()), TeleportDestination::Position(args.location))
            }
            Self::TargetPlayer(args) => {
                (Some(args.target.as_str()), TeleportDestination::Player(args.destination.as_str()))
            }
        };
        let (name, position) = instance.teleport_player(sender, target, destination).await?;
        Ok(format!("Teleported {name} to {}, {}, {}.", position.x, position.y, position.z))
    }
}
