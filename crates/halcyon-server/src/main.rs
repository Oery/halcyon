use futures_lite::stream::StreamExt;
use glommio::net::TcpListener;
use std::sync::Arc;

use crate::client::Client;
use crate::server::Server;

mod auth;
mod cache;
mod chunks;
mod client;
mod encryption;
mod error;
mod lifecycle;
mod lobby;
mod packet;
mod server;
mod states;
mod transport;
mod visibility;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
static ADDR: &str = "127.0.0.1:25565";

#[glommio::main(placement = Fixed(0))]
async fn main() -> std::io::Result<()> {
    let lobby = halcyon_world::World::load(std::path::Path::new("worlds/lobby"))
        .map_err(std::io::Error::other)?;
    let server = Arc::new(Server::new(lobby).await.map_err(std::io::Error::other)?);
    let players = lobby::Lobby::spawn(server.diagnostics);

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
