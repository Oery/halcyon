//! Moving a connection between instances on this executor. The client sees a
//! dimension change: it discards the old level, then loads the new one.

use mc_protocol::packets::client::{PlayerInfoRemove, Respawn, SystemChatMessage};
use mc_protocol::types::Nbt;

use super::Client;
use crate::error::Error;
use crate::game::Destination;

impl Client {
    pub async fn move_to(&mut self, destination: Destination) -> Result<(), Error> {
        let entity_id = self.entity_id()?;
        let Some(target) = self.instances.resolve(destination) else {
            return self.notify_player("That game is unavailable right now.").await;
        };
        if target.id() == self.instance.id() {
            return Ok(());
        }
        // Leaving: the old owner removes the player on its next pass.
        self.session = None;
        let roster = self.observer.roster_uuids();
        if !roster.is_empty() {
            self.send(&PlayerInfoRemove { uuids: &roster }).await?;
        }
        self.observer = Default::default();
        self.world_observer = Default::default();
        self.effects = Default::default();
        self.chunks.clear_residency();
        self.camera = None;
        self.pending_teleport = None;
        self.teleport_deadline = None;
        self.sneaking = false;
        self.sprinting = false;
        self.game_mode = Default::default();
        let id = target.id();
        self.instance = target;
        self.dimension = format!("halcyon:instance_{}_{}", id.shard, id.local);
        let dimension = self.dimension.clone();
        self.send(&Respawn {
            dimension_type: 0,
            dimension_name: &dimension,
            hashed_seed: 0,
            game_mode: self.game_mode.current as i32,
            previous_game_mode: self.game_mode.previous.map_or(0, |mode| mode as i32 + 1),
            is_debug: false,
            is_flat: false,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            data_kept: 0,
        })
        .await?;
        // A full or stopped target sends the player to the hub instead.
        if let Err(error) = self.reserve_player(Some(entity_id)).await {
            let hub = self.instances.hub().filter(|hub| hub.id() != self.instance.id());
            let Some(hub) = hub else { return Err(error) };
            self.instance = hub;
            self.reserve_player(Some(entity_id)).await?;
        }
        crate::states::play::enter_instance(self).await
    }

    async fn notify_player(&mut self, text: &str) -> Result<(), Error> {
        self.send(&SystemChatMessage { content: Nbt::Str(text), overlay: false }).await
    }
}
