use halcyon_commands::CommandArgs;

use super::Reply;
use crate::game::Destination;

#[derive(Debug, CommandArgs)]
pub(super) struct HubCommand;

impl HubCommand {
    pub(super) fn execute(self) -> Reply {
        Reply { message: "Returning to the hub...".to_owned(), destination: Some(Destination::Hub) }
    }
}
