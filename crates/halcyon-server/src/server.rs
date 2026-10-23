use crate::encryption::Keys;

pub struct Server {
    pub keys: Keys,
    pub lobby: std::sync::Arc<halcyon_world::World>,
    pub packets: crate::cache::PacketCache,
    pub diagnostics: bool,
}

impl Server {
    pub async fn new(mut lobby: halcyon_world::World) -> Result<Server, crate::error::Error> {
        let (x, y, z, yaw, pitch) = lobby.spawn;
        let spawn = crate::instance::Transform { x, y, z, yaw, pitch, on_ground: false };
        let safe_spawn = crate::movement::resolve_spawn(&lobby, spawn)
            .ok_or(crate::error::Error::UnsupportedSpawn)?;
        if safe_spawn != spawn {
            eprintln!(
                "world: adjusted obstructed spawn from ({x}, {y}, {z}) to ({}, {}, {})",
                safe_spawn.x, safe_spawn.y, safe_spawn.z
            );
            lobby.spawn = (safe_spawn.x, safe_spawn.y, safe_spawn.z, yaw, pitch);
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
