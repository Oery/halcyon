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
mod commands;
mod effects;
mod encryption;
mod error;
mod interaction;
mod inventory;
mod lifecycle;
mod lobby;
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

#[glommio::main(placement = Fixed(0))]
async fn main() -> std::io::Result<()> {
    let lobby = halcyon_world::World::load(std::path::Path::new("worlds/Parkour Spiral"))
        .map_err(std::io::Error::other)?;
    let server = Arc::new(Server::new(lobby).await.map_err(std::io::Error::other)?);
    let players = lobby::Lobby::spawn(server.lobby.clone(), server.diagnostics)
        .map_err(std::io::Error::other)?;

    // Temporary until matches own bot populations: HALCYON_BOTS=<count>.
    let bots = std::env::var("HALCYON_BOTS").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
    bot::spawn(server.clone(), players.clone(), bots, 0x0068_616c_6379_6f6e);

    let listener = TcpListener::bind(ADDR)?;
    let mut incoming = listener.incoming();

    println!("Listening on {}", listener.local_addr()?);

    while let Some(conn) = incoming.next().await {
        match conn {
            Ok(conn) => {
                let handle = Arc::clone(&server);
                let players = players.clone();
                glommio::spawn_local(async move {
                    Client::new(conn, handle, players).run().await;
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
