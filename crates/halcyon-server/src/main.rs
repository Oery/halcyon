use futures_lite::stream::StreamExt;
use glommio::net::TcpListener;
use std::sync::Arc;

use crate::client::Client;
use crate::server::Server;

mod auth;
mod client;
mod encryption;
mod packet;
mod server;
mod states;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
static ADDR: &str = "127.0.0.1:25565";

#[glommio::main(placement = Fixed(0))]
async fn main() {
    let server = Arc::new(Server::new());

    let listener = TcpListener::bind(ADDR).unwrap();
    let mut incoming = listener.incoming();

    println!("Listening on {}", listener.local_addr().unwrap());

    while let Some(Ok(conn)) = incoming.next().await {
        println!("connection!");

        let handle = Arc::clone(&server);
        glommio::spawn_local(async move {
            Client::new(conn, handle).run().await;
        })
        .detach();
    }

    eprintln!("error: could not accept client connection");
}
