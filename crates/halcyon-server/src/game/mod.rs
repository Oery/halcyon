//! Minigame logic attached to an instance. The instance owner runs a game
//! synchronously: `on_join` during admission, events and `tick` after every
//! simulation step, all with direct access to the instance's players.
//! Round-based games implement `Match` instead and run inside `MatchRunner`.
use std::sync::Arc;

use crate::instance::{GameCtx, InstanceId};
use crate::map::Map;
use crate::server::Server;

pub(crate) mod boxes;
mod ffa;
mod hub;
mod ladder;
pub(crate) mod matches;

pub use boxes::Boxes;
pub use ffa::Ffa;
pub use hub::Hub;
pub use ladder::{Ladder, Sparring};
pub use matches::MatchRunner;

/// Announced to everyone when a combat round starts.
pub const FIGHT: &str = "Fight!";

/// Game kinds players can ask for by name.
pub const KINDS: [&str; 5] = [Hub::KIND, Ffa::KIND, Boxes::KIND, Boxes::LONG_KIND, Ladder::KIND];

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

    /// Runs once when the instance starts, before anyone can join: build
    /// the game's terrain here.
    fn on_create(&mut self, _ctx: &mut GameCtx<'_>) {}

    /// A game that brings its own bot: the instance starts one with this.
    fn sparring(&self) -> Option<std::rc::Rc<Sparring>> {
        None
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
        Ffa::KIND => Some((Box::new(MatchRunner::new(Ffa::default())), ffa_map(server)?.clone())),
        Boxes::KIND | Boxes::LONG_KIND => {
            let map = boxes_map(server)?;
            let kit = if kind == Boxes::KIND {
                boxes::Kit::Iron
            } else {
                boxes::Kit::Long
            };
            let kind = if kind == Boxes::KIND {
                Boxes::KIND
            } else {
                Boxes::LONG_KIND
            };
            let Some(game) = Boxes::new(&map.world, kind, kit) else {
                eprintln!("boxes: no room for the arena over map {}", map.name);
                return None;
            };
            Some((Box::new(MatchRunner::new(game)), map.clone()))
        }
        Ladder::KIND => {
            use std::sync::atomic::{AtomicU32, Ordering};
            static RIVALS: AtomicU32 = AtomicU32::new(1);
            let map = boxes_map(server)?;
            let name = format!("Rival{}", RIVALS.fetch_add(1, Ordering::Relaxed));
            let Some(game) = Ladder::new(&map.world, name) else {
                eprintln!("ladder: no room for the arena over map {}", map.name);
                return None;
            };
            Some((Box::new(game), map.clone()))
        }
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

fn boxes_map(server: &Server) -> Option<&Arc<Map>> {
    server.maps.get(&std::env::var("HALCYON_BOXES_MAP").unwrap_or_else(|_| boxes::MAP.to_owned()))
}
