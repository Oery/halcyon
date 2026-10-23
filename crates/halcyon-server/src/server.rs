use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::encryption::Keys;
use crate::map::MapCatalog;

pub struct Server {
    pub keys: Keys,
    pub maps: MapCatalog,
    /// One entity ID space for every instance on every executor.
    pub entity_ids: crate::instance::EntityIds,
    /// Accounts with a connection in Play, whichever instance it is in.
    accounts: Mutex<HashSet<Uuid>>,
    pub packets: crate::cache::PacketCache,
    pub diagnostics: bool,
}

impl Server {
    pub async fn new(maps: MapCatalog) -> Result<Server, crate::error::Error> {
        Ok(Server {
            keys: Keys::new()?,
            maps,
            entity_ids: Default::default(),
            accounts: Default::default(),
            packets: crate::cache::PacketCache::new().await?,
            diagnostics: std::env::var_os("HALCYON_DIAGNOSTICS").is_some(),
        })
    }

    /// None while another connection already plays on this account.
    pub fn claim_account(self: &Arc<Self>, uuid: Uuid) -> Option<Account> {
        let mut accounts = self.accounts.lock().unwrap_or_else(|poison| poison.into_inner());
        accounts.insert(uuid).then(|| Account { server: self.clone(), uuid })
    }
}

/// Holds an account's place until its connection ends.
pub struct Account {
    server: Arc<Server>,
    uuid: Uuid,
}

impl Drop for Account {
    fn drop(&mut self) {
        let mut accounts = self.server.accounts.lock().unwrap_or_else(|poison| poison.into_inner());
        accounts.remove(&self.uuid);
    }
}
