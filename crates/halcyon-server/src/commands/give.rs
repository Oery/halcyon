use halcyon_commands::{ArgumentKind, CommandArgs, CommandArgument};
use mc_protocol::data::items::ITEMS;
use mc_protocol::types::ItemStack;

use super::CommandError;
use crate::lobby::Lobby;

#[derive(Debug)]
pub(super) struct ItemName<'p>(&'p str);

impl<'p> CommandArgument<'p> for ItemName<'p> {
    const KIND: ArgumentKind = ArgumentKind::Word { suggestions: Some("items") };
    fn parse(value: &'p str) -> Result<Self, &'static str> {
        if value.len() <= 128
            && value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b':'))
        {
            Ok(Self(value))
        } else {
            Err("Expected a supported item identifier.")
        }
    }
}

pub(super) fn allowed(id: usize) -> bool {
    let name = ITEMS[id].0;
    crate::interaction::placement_item(id as i32).is_some()
        || name.starts_with("minecraft:music_disc_")
        || ["_pickaxe", "_axe", "_shovel", "_hoe", "_sword"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || name == "minecraft:shears"
}

pub(super) fn suggestions(prefix: &str) -> Vec<String> {
    ITEMS
        .iter()
        .enumerate()
        .filter(|(id, _)| allowed(*id))
        .map(|(_, (name, ..))| {
            if prefix.contains(':') {
                *name
            } else {
                name.strip_prefix("minecraft:").unwrap_or(name)
            }
        })
        .filter(|name| name.starts_with(prefix))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, CommandArgs)]
pub(super) struct GiveCommand<'p> {
    item: ItemName<'p>,
    count: Option<i32>,
}

impl GiveCommand<'_> {
    pub(super) async fn execute(self, lobby: &Lobby, sender: i32) -> Result<String, CommandError> {
        let name = self.item.0.strip_prefix("minecraft:").unwrap_or(self.item.0);
        let id = ITEMS
            .iter()
            .position(|(id, ..)| id.strip_prefix("minecraft:") == Some(name))
            .filter(|id| allowed(*id))
            .ok_or(CommandError::Message("Unsupported item."))?;
        let count = u8::try_from(self.count.unwrap_or(1))
            .map_err(|_| CommandError::Message("Invalid item count."))?;
        let stack = ItemStack::plain(id as i32, count).map_err(|_| {
            CommandError::Message("Count must be between 1 and the item's stack limit.")
        })?;
        let remainder = lobby.give_creative(sender, stack).await?;
        Ok(format!(
            "Gave {} {name}.{}",
            count - remainder.count(),
            if remainder.is_empty() {
                ""
            } else {
                " Inventory full; remaining items were not granted."
            }
        ))
    }
}
