//! Minigame logic attached to an instance. The instance owner runs a game
//! synchronously: `on_join` during admission, events and `tick` after every
//! simulation step, all with direct access to the instance's players.
use std::sync::Arc;

use crate::instance::{GameCtx, InstanceId};
use crate::map::Map;
use crate::server::Server;

mod ffa;
mod hub;

pub use ffa::Ffa;
pub use hub::Hub;

/// Game kinds players can ask for by name.
pub const KINDS: [&str; 2] = [Hub::KIND, Ffa::KIND];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameEvent {
    /// A player's session ended, by disconnecting or moving to another instance.
    Left(i32),
    Died {
        player: i32,
        killer: Option<i32>,
    },
    Respawned(i32),
}

/// Matchmaking only places new players in `Open` instances.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Open,
    Running,
    Ending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Hub,
    /// Join an open instance of this game kind, or a new one.
    Matchmake(&'static str),
    #[allow(dead_code)]
    Instance(InstanceId),
}

pub trait Game {
    fn kind(&self) -> &'static str;

    /// Players admitted at once, including reservations still loading.
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn phase(&self) -> Phase {
        Phase::Open
    }

    /// Runs before the player's client receives its entity ID: place it and
    /// set its rules here. The player is not yet visible to others.
    fn on_join(&mut self, _ctx: &mut GameCtx<'_>, _player: i32) {}

    fn on_event(&mut self, _ctx: &mut GameCtx<'_>, _event: GameEvent) {}

    fn tick(&mut self, _ctx: &mut GameCtx<'_>) {}
}

/// A new game of `kind` and the map it plays on.
pub fn create(kind: &str, server: &Server) -> Option<(Box<dyn Game>, Arc<Map>)> {
    match kind {
        Hub::KIND => Some((Box::new(Hub::from_env(false)), hub_map(server)?.clone())),
        Ffa::KIND => Some((Box::new(Ffa::default()), ffa_map(server)?.clone())),
        _ => None,
    }
}

pub fn kind(name: &str) -> Option<&'static str> {
    KINDS.into_iter().find(|kind| *kind == name)
}

pub fn hub_map(server: &Server) -> Option<&Arc<Map>> {
    server.maps.get(&std::env::var("HALCYON_HUB_MAP").unwrap_or_else(|_| hub::MAP.to_owned()))
}

fn ffa_map(server: &Server) -> Option<&Arc<Map>> {
    server.maps.get(&std::env::var("HALCYON_FFA_MAP").unwrap_or_else(|_| ffa::MAP.to_owned()))
}
