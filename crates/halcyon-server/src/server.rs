use crate::encryption::Keys;

pub struct Server {
    pub keys: Keys,
    pub lobby: halcyon_world::World,
    pub packets: crate::cache::PacketCache,
}

impl Server {
    pub async fn new(lobby: halcyon_world::World) -> Result<Server, crate::error::Error> {
        let packets = crate::cache::PacketCache::new(&lobby).await?;
        Ok(Server {
            keys: Keys::new()?,
            lobby,
            packets,
        })
    }
}
