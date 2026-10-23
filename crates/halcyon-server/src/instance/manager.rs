//! The instances one executor owns: creation, matchmaking and reaping.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use super::{Instance, InstanceId};
use crate::game::{Destination, Hub};
use crate::server::Server;

struct Instances {
    server: Arc<Server>,
    shard: u16,
    next: u32,
    instances: BTreeMap<u32, Instance>,
    hub: Option<InstanceId>,
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

    /// Starts a new instance of `kind` on its configured map.
    pub fn create(&self, kind: &str) -> Option<Instance> {
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
        Some(instance)
    }

    pub fn get(&self, id: InstanceId) -> Option<Instance> {
        let mut inner = self.inner.borrow_mut();
        inner.reap();
        inner.instances.get(&id.local).filter(|_| id.shard == inner.shard).cloned()
    }

    /// The running hub, restarted if it ever stopped.
    pub fn hub(&self) -> Option<Instance> {
        let hub = self.inner.borrow().hub;
        if let Some(hub) = hub.and_then(|id| self.get(id)) {
            return Some(hub);
        }
        let hub = self.create(Hub::KIND)?;
        self.inner.borrow_mut().hub = Some(hub.id());
        Some(hub)
    }

    /// An open instance of `kind` with room, oldest first, or a new one.
    pub fn matchmake(&self, kind: &str) -> Option<Instance> {
        if kind == Hub::KIND {
            return self.hub();
        }
        let open = {
            let mut inner = self.inner.borrow_mut();
            inner.reap();
            inner
                .instances
                .values()
                .find(|instance| instance.kind() == kind && instance.has_room())
                .cloned()
        };
        open.or_else(|| self.create(kind))
    }

    pub fn resolve(&self, destination: Destination) -> Option<Instance> {
        match destination {
            Destination::Hub => self.hub(),
            Destination::Matchmake(kind) => self.matchmake(kind),
            Destination::Instance(id) => self.get(id),
        }
    }

    /// Visible players across this executor's instances.
    pub fn online(&self) -> usize {
        self.inner.borrow().instances.values().map(Instance::active_count).sum()
    }
}

impl Instances {
    fn reap(&mut self) {
        self.instances.retain(|_, instance| !instance.closed());
    }
}
