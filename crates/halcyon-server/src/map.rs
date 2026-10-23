//! Immutable map templates. Every instance of a map shares its terrain and
//! encoded chunks; instances only add their own `Arena` overlay.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::cache::ChunkCache;
use crate::error::Error;

pub struct Map {
    pub name: String,
    pub world: halcyon_world::World,
    pub chunks: ChunkCache,
}

impl Map {
    pub async fn new(name: String, mut world: halcyon_world::World) -> Result<Self, Error> {
        let (x, y, z, yaw, pitch) = world.spawn;
        let spawn = crate::instance::Transform { x, y, z, yaw, pitch, on_ground: false };
        let safe_spawn =
            crate::movement::resolve_spawn(&world, spawn).ok_or(Error::UnsupportedSpawn)?;
        if safe_spawn != spawn {
            eprintln!(
                "map {name}: adjusted obstructed spawn from ({x}, {y}, {z}) to ({}, {}, {})",
                safe_spawn.x, safe_spawn.y, safe_spawn.z
            );
            world.spawn = (safe_spawn.x, safe_spawn.y, safe_spawn.z, yaw, pitch);
        }
        let chunks = ChunkCache::new(&world).await?;
        Ok(Self { name, world, chunks })
    }

    pub fn spawn(&self) -> crate::instance::Transform {
        let (x, y, z, yaw, pitch) = self.world.spawn;
        crate::instance::Transform { x, y, z, yaw, pitch, on_ground: false }
    }
}

/// Maps loaded once at startup, shared by every shard.
#[derive(Default)]
pub struct MapCatalog {
    maps: BTreeMap<String, Arc<Map>>,
}

impl MapCatalog {
    /// Loads every world folder in `dir`; a folder that fails to load is skipped.
    pub async fn load(dir: &Path) -> Result<Self, Error> {
        let mut catalog = Self::default();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let world = match halcyon_world::World::load(&path) {
                Ok(world) => world,
                Err(error) => {
                    eprintln!("map {name}: skipped: {error}");
                    continue;
                }
            };
            catalog.insert(Map::new(name, world).await?);
        }
        Ok(catalog)
    }

    pub fn insert(&mut self, map: Map) {
        self.maps.insert(map.name.clone(), Arc::new(map));
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Map>> {
        self.maps.get(name)
    }
}
