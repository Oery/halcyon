//! Moving a connection between instances. The client sees a dimension
//! change: it discards the old level, then loads the new one. An instance on
//! another shard takes the whole connection there, socket and cipher state
//! included, so the player's gameplay stays on one core.

use std::rc::Rc;
use std::sync::Arc;

use glommio::net::{AcceptedTcpStream, TcpStream};
use mc_protocol::packets::Context;
use mc_protocol::packets::client::{PlayerInfoRemove, Respawn, SystemChatMessage};
use mc_protocol::types::Nbt;

use super::Client;
use crate::chunks::ChunkTracker;
use crate::error::Error;
use crate::game::Destination;
use crate::instance::{Instance, InstanceManager, Route};
use crate::lifecycle::{Keepalive, LoginProgress};
use crate::server::{Account, Server};
use crate::shard::{Shard, ShardMsg};
use crate::transport::ClientStream;

pub(super) struct Handoff {
    shard: u16,
    destination: Destination,
    entity_id: i32,
}

/// A connection between shards, after its player left the old instance and
/// the client was told to discard the old level.
pub struct Parked {
    stream: ClientStream<AcceptedTcpStream>,
    ctx: Context,
    login: LoginProgress,
    keepalive: Keepalive,
    chunks: ChunkTracker,
    account: Option<Account>,
    profile_info: Box<[u8]>,
    dimension: String,
    levels: u32,
    entity_id: i32,
    destination: Destination,
    /// Received but not yet handled; the new shard handles these first.
    unread: Vec<u8>,
}

impl Client {
    pub async fn move_to(&mut self, destination: Destination) -> Result<(), Error> {
        if self.handoff.is_some() {
            return Ok(());
        }
        match self.instances.route(destination) {
            Route::Unavailable => self.notify_player("That game is unavailable right now.").await,
            Route::Local(target) if target.id() == self.instance.id() => Ok(()),
            Route::Local(target) => {
                let entity_id = self.leave_instance().await?;
                self.join_instance(target, entity_id).await
            }
            Route::Remote(shard, destination) => {
                let entity_id = self.leave_instance().await?;
                // The connection loop parks the client before handling more input.
                self.handoff = Some(Handoff { shard, destination, entity_id });
                Ok(())
            }
        }
    }

    /// Ends the session and clears what the client knows of this instance.
    /// Returns the entity ID, which the player keeps in its next instance.
    async fn leave_instance(&mut self) -> Result<i32, Error> {
        let entity_id = self.entity_id()?;
        // The old owner removes the player on its next pass.
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
        self.levels += 1;
        self.dimension = format!("halcyon:level_{}", self.levels);
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
        Ok(entity_id)
    }

    /// Admits the player to `target`, or to this shard's hub if `target` is
    /// full or stopped, after `leave_instance`.
    async fn join_instance(&mut self, target: Instance, entity_id: i32) -> Result<(), Error> {
        self.instance = target;
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

    /// Sends the parked connection to its shard. If that shard cannot take
    /// it, the player joins this shard's hub instead.
    pub(super) fn hand_off(self, unread: Vec<u8>) {
        let Some(handoff) = self.handoff else { return };
        let Some(profile_info) = self.profile_info else { return };
        let shard = self.shard;
        let parked = Parked {
            stream: self.stream.map_inner(TcpStream::into_accepted),
            ctx: self.ctx,
            login: self.login,
            keepalive: self.keepalive,
            chunks: self.chunks,
            account: self.account,
            profile_info: Box::from(&*profile_info),
            dimension: self.dimension,
            levels: self.levels,
            entity_id: handoff.entity_id,
            destination: handoff.destination,
            unread,
        };
        if let Err(ShardMsg::Handoff(mut parked)) =
            shard.send(handoff.shard, ShardMsg::Handoff(Box::new(parked)))
        {
            parked.destination = Destination::Hub;
            let client = Client::resume(*parked, &self.server, &shard, &self.instances);
            glommio::spawn_local(client).detach();
        }
    }

    /// Continues a parked connection on this shard: admit, then serve it.
    pub fn resume(
        parked: Parked,
        server: &Arc<Server>,
        shard: &Rc<Shard>,
        instances: &InstanceManager,
    ) -> impl Future<Output = ()> + 'static {
        let server = server.clone();
        let shard = shard.clone();
        let instances = instances.clone();
        async move {
            let Some(hub) = instances.hub() else { return };
            let stream = parked.stream.map_inner(AcceptedTcpStream::bind_to_executor);
            let mut client = Client::with_stream(stream, server, shard, instances, hub.clone());
            client.ctx = parked.ctx;
            client.login = parked.login;
            client.keepalive = parked.keepalive;
            client.chunks = parked.chunks;
            client.account = parked.account;
            client.profile_info = Some(Rc::from(parked.profile_info));
            client.dimension = parked.dimension;
            client.levels = parked.levels;
            let target = client.instances.resolve(parked.destination).unwrap_or(hub);
            if let Err(error) = client.join_instance(target, parked.entity_id).await {
                if client.server.diagnostics {
                    eprintln!("dropping connection after handoff: {error}");
                }
                return;
            }
            client.run_from(parked.unread).await;
        }
    }
}
