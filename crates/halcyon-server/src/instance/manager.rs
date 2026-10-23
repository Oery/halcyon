//! The instances one shard owns: creation, matchmaking, routing and reaping.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use super::{Instance, InstanceId};
use crate::game::{Destination, Hub};
use crate::server::{Server, ShardSummary};

struct Instances {
    server: Arc<Server>,
    shard: u16,
    next: u32,
    instances: BTreeMap<u32, Instance>,
    hub: Option<InstanceId>,
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
        let inner = Instances { server, shard, next: 0, instances: BTreeMap::new(), hub: None };
        Self { inner: Rc::new(RefCell::new(inner)) }
    }

    pub fn shard(&self) -> u16 {
        self.inner.borrow().shard
    }

    /// Starts a new instance of `kind` on its configured map.
    pub fn create(&self, kind: &str) -> Option<Instance> {
        let instance = {
            let mut inner = self.inner.borrow_mut();
            let (game, map) = crate::game::create(kind, &inner.server)?;
            let id = InstanceId { shard: inner.shard, local: inner.next };
            let ids = inner.server.entity_ids.clone();
            let instance = match Instance::spawn(id, map, game, ids, inner.server.diagnostics) {
                Ok(instance) => instance,
                Err(error) => {
                    eprintln!("instance {id} ({kind}): failed to start: {error}");
                    return None;
                }
            };
            inner.next += 1;
            inner.instances.insert(id.local, instance.clone());
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

    /// This shard's hub, restarted if it ever stopped. Every shard has one.
    pub fn hub(&self) -> Option<Instance> {
        let hub = self.inner.borrow().hub;
        if let Some(hub) = hub.and_then(|id| self.get(id)) {
            return Some(hub);
        }
        let hub = self.create(Hub::KIND)?;
        self.inner.borrow_mut().hub = Some(hub.id());
        Some(hub)
    }

    fn open(&self, kind: &str) -> Option<Instance> {
        let mut inner = self.inner.borrow_mut();
        inner.reap();
        inner
            .instances
            .values()
            .find(|instance| instance.kind() == kind && instance.has_room())
            .cloned()
    }

    /// An open instance of `kind` here with room, oldest first, or a new one.
    pub fn matchmake(&self, kind: &str) -> Option<Instance> {
        if kind == Hub::KIND {
            return self.hub();
        }
        self.open(kind).or_else(|| self.create(kind))
    }

    /// Resolves a destination on this shard only.
    pub fn resolve(&self, destination: Destination) -> Option<Instance> {
        match destination {
            Destination::Hub => self.hub(),
            Destination::Matchmake(kind) => self.matchmake(kind),
            Destination::Instance(id) => self.get(id),
        }
    }

    /// Resolves a destination across shards: an open game anywhere first,
    /// then a new one on the least-loaded shard, preferring this one on ties.
    pub fn route(&self, destination: Destination) -> Route {
        let shard = self.shard();
        let local = |instance: Option<Instance>| instance.map_or(Route::Unavailable, Route::Local);
        match destination {
            Destination::Instance(id) if id.shard != shard => Route::Remote(id.shard, destination),
            Destination::Matchmake(kind) if kind != Hub::KIND => {
                if let Some(open) = self.open(kind) {
                    return Route::Local(open);
                }
                let server = self.inner.borrow().server.clone();
                let target = server.directory.open(kind).filter(|&other| other != shard);
                match target.unwrap_or_else(|| self.placement(&server)) {
                    target if target == shard => local(self.create(kind)),
                    target => Route::Remote(target, destination),
                }
            }
            _ => local(self.resolve(destination)),
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
        let summary = {
            let mut inner = self.inner.borrow_mut();
            inner.reap();
            let mut open: Vec<&'static str> = Vec::new();
            for instance in inner.instances.values() {
                if instance.has_room() && !open.contains(&instance.kind()) {
                    open.push(instance.kind());
                }
            }
            ShardSummary { players: 0, open }
        };
        let summary = ShardSummary { players: self.online(), ..summary };
        let inner = self.inner.borrow();
        inner.server.directory.publish(inner.shard, summary);
    }
}

impl Instances {
    fn reap(&mut self) {
        self.instances.retain(|_, instance| !instance.closed());
    }
}
