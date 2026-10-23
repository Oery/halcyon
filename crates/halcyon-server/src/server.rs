use crate::encryption::Keys;
use crate::map::MapCatalog;

pub struct Server {
    pub keys: Keys,
    pub maps: MapCatalog,
    pub packets: crate::cache::PacketCache,
    pub diagnostics: bool,
}

impl Server {
    pub async fn new(maps: MapCatalog) -> Result<Server, crate::error::Error> {
        Ok(Server {
            keys: Keys::new()?,
            maps,
            packets: crate::cache::PacketCache::new().await?,
            diagnostics: std::env::var_os("HALCYON_DIAGNOSTICS").is_some(),
        })
    }
}
