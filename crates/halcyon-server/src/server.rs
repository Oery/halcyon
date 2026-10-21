use crate::encryption::Keys;

pub struct Server {
    pub keys: Keys,
    pub lobby: std::sync::Arc<halcyon_world::World>,
    pub packets: crate::cache::PacketCache,
    pub diagnostics: bool,
}

impl Server {
    pub async fn new(lobby: halcyon_world::World) -> Result<Server, crate::error::Error> {
        let (x, y, z, yaw, pitch) = lobby.spawn;
        let spawn = crate::lobby::Transform { x, y, z, yaw, pitch, on_ground: false };
        if !spawn.is_valid() {
            return Err(crate::error::Error::InvalidPlayerData);
        }
        let packets = crate::cache::PacketCache::new(&lobby).await?;
        Ok(Server {
            keys: Keys::new()?,
            lobby: std::sync::Arc::new(lobby),
            packets,
            diagnostics: std::env::var_os("HALCYON_DIAGNOSTICS").is_some(),
        })
    }
}
