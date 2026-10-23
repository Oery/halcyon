use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use uuid::Uuid;

use crate::encryption::Keys;
use crate::map::MapCatalog;

/// Immutable startup resources and the little state every shard shares.
pub struct Server {
    pub keys: Keys,
    pub maps: MapCatalog,
    /// One entity ID space for every instance on every executor.
    pub entity_ids: crate::instance::EntityIds,
    /// Accounts with a connection in Play, whichever instance it is in.
    accounts: Mutex<HashSet<Uuid>>,
    pub directory: Directory,
    pub packets: crate::cache::PacketCache,
    pub diagnostics: bool,
}

impl Server {
    pub async fn new(maps: MapCatalog, shards: usize) -> Result<Server, crate::error::Error> {
        Ok(Server {
            keys: Keys::new()?,
            maps,
            entity_ids: Default::default(),
            accounts: Default::default(),
            directory: Directory::new(shards),
            packets: crate::cache::PacketCache::new().await?,
            diagnostics: std::env::var_os("HALCYON_DIAGNOSTICS").is_some(),
        })
    }

    /// None while another connection already plays on this account.
    pub fn claim_account(self: &Arc<Self>, uuid: Uuid) -> Option<Account> {
        accounts(&self.accounts).insert(uuid).then(|| Account { server: self.clone(), uuid })
    }
}

fn accounts(accounts: &Mutex<HashSet<Uuid>>) -> MutexGuard<'_, HashSet<Uuid>> {
    accounts.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Holds an account's place until its connection ends, on any shard.
pub struct Account {
    server: Arc<Server>,
    uuid: Uuid,
}

impl Drop for Account {
    fn drop(&mut self) {
        accounts(&self.server.accounts).remove(&self.uuid);
    }
}

/// What a shard last published about itself. Placement reads it without
/// asking the shard, so it can be slightly stale; the target shard rechecks.
#[derive(Clone, Debug, Default)]
pub struct ShardSummary {
    pub players: usize,
    /// Game kinds with an instance here that is open and has room.
    pub open: Vec<&'static str>,
}

pub struct Directory {
    shards: Mutex<Box<[ShardSummary]>>,
}

impl Directory {
    fn new(shards: usize) -> Self {
        Self { shards: Mutex::new(vec![ShardSummary::default(); shards].into_boxed_slice()) }
    }

    fn lock(&self) -> MutexGuard<'_, Box<[ShardSummary]>> {
        self.shards.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn shards(&self) -> usize {
        self.lock().len()
    }

    pub fn publish(&self, shard: u16, summary: ShardSummary) {
        if let Some(entry) = self.lock().get_mut(usize::from(shard)) {
            *entry = summary;
        }
    }

    pub fn online(&self) -> usize {
        self.lock().iter().map(|shard| shard.players).sum()
    }

    /// The least-loaded shard with an open instance of `kind`, if any.
    pub fn open(&self, kind: &str) -> Option<u16> {
        let shards = self.lock();
        (0..shards.len())
            .filter(|&shard| shards[shard].open.contains(&kind))
            .min_by_key(|&shard| shards[shard].players)
            .map(|shard| shard as u16)
    }

    pub fn load(&self, shard: u16) -> usize {
        self.lock().get(usize::from(shard)).map_or(usize::MAX, |summary| summary.players)
    }

    /// Where a new instance goes: the shard with the fewest players.
    pub fn least_loaded(&self) -> u16 {
        let shards = self.lock();
        (0..shards.len()).min_by_key(|&shard| shards[shard].players).unwrap_or(0) as u16
    }
}

#[cfg(test)]
#[path = "tests/tests_server.rs"]
mod tests;
