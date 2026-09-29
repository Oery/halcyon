use futures_lite::stream::StreamExt;
use glommio::net::TcpListener;

use crate::client::Client;

mod client;
mod packet;
mod states;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
static ADDR: &str = "127.0.0.1:25565";

#[glommio::main(placement = Fixed(0))]
async fn main() {
    let listener = TcpListener::bind(ADDR).unwrap();
    let mut incoming = listener.incoming();

    println!("Listening on {}", listener.local_addr().unwrap());

    while let Some(Ok(conn)) = incoming.next().await {
        println!("connection!");

        glommio::spawn_local(async move {
            Client::new(conn).run().await;
        })
        .detach();
    }

    eprintln!("error: could not accept client connection");
}
