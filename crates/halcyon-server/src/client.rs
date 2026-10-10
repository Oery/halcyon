use futures_lite::AsyncReadExt;
use glommio::net::TcpStream;
use mc_protocol::packets::Context;
use std::sync::Arc;

use crate::packet::read_packet;
use crate::server::Server;
use crate::states::handle_packet;

const SIZE: usize = 1_024;

pub struct Client {
    pub stream: TcpStream,
    pub ctx: Context,
    pub server: Arc<Server>,
}

impl Client {
    pub fn new(stream: TcpStream, server: Arc<Server>) -> Client {
        Client { stream, ctx: Context::default(), server }
    }

    pub async fn run(&mut self) {
        let mut buf = [0u8; SIZE];
        let mut len = 0;

        self.stream.set_nodelay(true).expect("set_nodelay call failed");

        while let Ok(n) = self.stream.read(&mut buf[len..SIZE]).await
            && n > 0
        {
            println!("Buffer = {:?}", &buf[len..len + n]);

            len += n;

            let mut total_read = 0;

            while let Some((pkt, processed)) = read_packet(&buf[..len], self).unwrap() {
                handle_packet(pkt, self).await;
                total_read += processed;
            }

            buf.copy_within(total_read..len, 0);
            len -= total_read;
        }
    }
}
