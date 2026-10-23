use futures_lite::stream::StreamExt;
use glommio::net::TcpListener;
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
mod simulation;
mod states;
mod transport;
mod visibility;
mod world_sync;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
static ADDR: &str = "127.0.0.1:25565";
static WORLDS: &str = "worlds";

#[glommio::main(placement = Fixed(0))]
async fn main() -> std::io::Result<()> {
    let maps =
        map::MapCatalog::load(std::path::Path::new(WORLDS)).await.map_err(std::io::Error::other)?;
    let server = Arc::new(Server::new(maps).await.map_err(std::io::Error::other)?);
    // Phase one runs every instance on this single executor, shard 0.
    let instances = instance::InstanceManager::new(server.clone(), 0);
    let hub = instances.hub().ok_or_else(|| {
        std::io::Error::other(format!("the hub map is missing from {WORLDS}; set HALCYON_HUB_MAP"))
    })?;

    // Temporary until matches own bot populations: HALCYON_BOTS=<count>.
    let bots = std::env::var("HALCYON_BOTS").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
    bot::spawn(server.clone(), hub, bots, 0x0068_616c_6379_6f6e);

    let listener = TcpListener::bind(ADDR)?;
    let mut incoming = listener.incoming();

    println!("Listening on {}", listener.local_addr()?);

    while let Some(conn) = incoming.next().await {
        match conn {
            Ok(conn) => {
                let Some(hub) = instances.hub() else {
                    eprintln!("hub unavailable; dropping connection");
                    continue;
                };
                let handle = Arc::clone(&server);
                let instances = instances.clone();
                glommio::spawn_local(async move {
                    Client::new(conn, handle, instances, hub).run().await;
                })
                .detach();
            }
            Err(e) => {
                eprintln!("accept failed: {e}");
                continue;
            }
        }
    }

    Ok(())
}
