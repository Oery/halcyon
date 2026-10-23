use glommio::{LocalExecutorPoolBuilder, PoolPlacement};
use std::sync::Arc;

use crate::client::Client;
use crate::server::Server;

mod arena;
mod auth;
mod bot;
mod cache;
mod chunks;
mod client;
mod combat;
mod commands;
mod effects;
mod encryption;
mod error;
mod game;
mod instance;
mod interaction;
mod inventory;
mod lifecycle;
mod map;
mod movement;
mod packet;
mod player;
mod server;
mod shard;
mod simulation;
mod states;
mod transport;
mod visibility;
mod world_sync;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
static ADDR: &str = "127.0.0.1:25565";
static WORLDS: &str = "worlds";

fn main() -> std::io::Result<()> {
    // Shards bind with SO_REUSEPORT, which would silently share the port with
    // another running server. A plain bind fails while any listener holds it.
    drop(std::net::TcpListener::bind(ADDR).map_err(|error| {
        std::io::Error::new(error.kind(), format!("{ADDR} is already in use: {error}"))
    })?);
    let shards = shard::count();
    // Maps and wire caches load once; every shard shares them read-only.
    let server = futures_lite::future::block_on(async {
        let maps = map::MapCatalog::load(std::path::Path::new(WORLDS)).await?;
        Server::new(maps, shards).await
    })
    .map_err(std::io::Error::other)?;
    if game::hub_map(&server).is_none() {
        return Err(std::io::Error::other(format!(
            "the hub map is missing from {WORLDS}; set HALCYON_HUB_MAP"
        )));
    }
    let server = Arc::new(server);
    let mesh = shard::Mesh::full(shards, shard::HANDOFF_CAPACITY);
    LocalExecutorPoolBuilder::new(PoolPlacement::MaxSpread(shards, None))
        .on_all_shards(move || async move { shard::run(server, mesh, ADDR).await })
        .map_err(std::io::Error::other)?
        .join_all();
    Ok(())
}
