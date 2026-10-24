//! The instances one shard owns: creation, matchmaking, routing and reaping.
//! Matchmaking fills the fullest instance of a kind that has room, on any
//! shard, before starting another on the least-loaded shard.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use super::{Instance, InstanceId};
use crate::game::{Destination, Game, Hub};
use crate::map::Map;
use crate::server::{Server, ShardSummary};

struct Instances {
    server: Arc<Server>,
    shard: u16,
    next: u32,
    instances: BTreeMap<u32, Instance>,
    home: Option<InstanceId>,
}

/// Where a connection should go to reach a destination.
pub enum Route {
    Local(Instance),
    /// Hand the connection to another shard, which resolves the destination itself.
    Remote(u16, Destination),
    Unavailable,
}

#[derive(Clone)]
pub struct InstanceManager {
    inner: Rc<RefCell<Instances>>,
}

impl InstanceManager {
    pub fn new(server: Arc<Server>, shard: u16) -> Self {
        let inner = Instances { server, shard, next: 0, instances: BTreeMap::new(), home: None };
        Self { inner: Rc::new(RefCell::new(inner)) }
    }

    pub fn shard(&self) -> u16 {
        self.inner.borrow().shard
    }

    /// Starts a new instance of `kind` on its configured map.
    pub fn create(&self, kind: &str) -> Option<Instance> {
        let (game, map) = crate::game::create(kind, &self.inner.borrow().server)?;
        self.start(game, map)
    }

    fn start(&self, game: Box<dyn Game>, map: Arc<Map>) -> Option<Instance> {
        let instance = {
            let mut inner = self.inner.borrow_mut();
            let id = InstanceId { shard: inner.shard, local: inner.next };
            let ids = inner.server.entity_ids.clone();
            let kind = game.kind();
            let sparring = game.sparring();
            let instance = match Instance::spawn(id, map, game, ids, inner.server.diagnostics) {
                Ok(instance) => instance,
                Err(error) => {
                    eprintln!("instance {id} ({kind}): failed to start: {error}");
                    return None;
                }
            };
            inner.next += 1;
            inner.instances.insert(id.local, instance.clone());
            if let Some(sparring) = sparring {
                crate::bot::spar(instance.clone(), sparring);
            }
            instance
        };
        // Other shards matchmaking at the same moment should find it.
        self.publish();
        Some(instance)
    }

    pub fn get(&self, id: InstanceId) -> Option<Instance> {
        let mut inner = self.inner.borrow_mut();
        inner.reap();
        inner.instances.get(&id.local).filter(|_| id.shard == inner.shard).cloned()
    }

    /// The hub this shard never stops, restarted if it ever did. A new
    /// connection belongs to it until login places the player.
    pub fn home(&self) -> Option<Instance> {
        let home = self.inner.borrow().home;
        if let Some(home) = home.and_then(|id| self.get(id)) {
            return Some(home);
        }
        let map = crate::game::hub_map(&self.inner.borrow().server)?.clone();
        let home = self.start(Box::new(Hub::from_env(true)), map)?;
        self.inner.borrow_mut().home = Some(home.id());
        Some(home)
    }

    /// The fullest hub here with room, or a new one.
    pub fn hub(&self) -> Option<Instance> {
        self.matchmake(Hub::KIND)
    }

    /// The fullest instance of `kind` here that has room.
    fn fullest(&self, kind: &str) -> Option<Instance> {
        if kind == Hub::KIND {
            // Keep the home hub running before anything else is considered.
            self.home();
        }
        let mut inner = self.inner.borrow_mut();
        inner.reap();
        inner
            .instances
            .values()
            .filter(|instance| instance.kind() == kind && instance.has_room())
            .max_by_key(|instance| instance.population())
            .cloned()
    }

    /// The fullest instance of `kind` here with room, or a new one.
    pub fn matchmake(&self, kind: &str) -> Option<Instance> {
        self.fullest(kind).or_else(|| self.create(kind))
    }

    /// Resolves a destination on this shard only.
    pub fn resolve(&self, destination: Destination) -> Option<Instance> {
        match destination {
            Destination::Hub => self.hub(),
            Destination::Matchmake(kind) => self.matchmake(kind),
            Destination::Instance(id) => self.get(id),
        }
    }

    /// Resolves a destination across shards: the fullest open instance of its
    /// kind anywhere (this shard on ties), else a new one on the least-loaded
    /// shard (this shard on ties).
    pub fn route(&self, destination: Destination) -> Route {
        let shard = self.shard();
        let kind = match destination {
            Destination::Instance(id) if id.shard != shard => {
                return Route::Remote(id.shard, destination);
            }
            Destination::Instance(id) => {
                return self.get(id).map_or(Route::Unavailable, Route::Local);
            }
            Destination::Hub => Hub::KIND,
            Destination::Matchmake(kind) => kind,
        };
        let remote = Destination::Matchmake(kind);
        let server = self.inner.borrow().server.clone();
        let local = self.fullest(kind);
        match (local, server.directory.fullest(kind, shard)) {
            (Some(local), Some((other, players))) if players > local.population() => {
                Route::Remote(other, remote)
            }
            (Some(local), _) => Route::Local(local),
            (None, Some((other, _))) => Route::Remote(other, remote),
            (None, None) => match self.placement(&server) {
                target if target == shard => {
                    self.create(kind).map_or(Route::Unavailable, Route::Local)
                }
                target => Route::Remote(target, remote),
            },
        }
    }

    fn placement(&self, server: &Server) -> u16 {
        let shard = self.shard();
        let least = server.directory.least_loaded();
        if self.online() <= server.directory.load(least) {
            shard
        } else {
            least
        }
    }

    /// Visible players across this shard's instances.
    pub fn online(&self) -> usize {
        self.inner.borrow().instances.values().map(Instance::active_count).sum()
    }

    pub fn publish(&self) {
        let players = self.online();
        let inner = &mut *self.inner.borrow_mut();
        inner.reap();
        let mut open: Vec<(&'static str, usize)> = Vec::new();
        for instance in inner.instances.values().filter(|instance| instance.has_room()) {
            let population = instance.population();
            match open.iter_mut().find(|(kind, _)| *kind == instance.kind()) {
                Some((_, fullest)) => *fullest = (*fullest).max(population),
                None => open.push((instance.kind(), population)),
            }
        }
        inner.server.directory.publish(inner.shard, ShardSummary { players, open });
    }
}

impl Instances {
    fn reap(&mut self) {
        self.instances.retain(|_, instance| !instance.closed());
    }
}
