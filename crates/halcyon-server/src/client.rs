use futures_lite::AsyncReadExt;
use glommio::net::TcpStream;
use mc_protocol::packets::Context;
use mc_protocol::state::State;

use crate::packet::read_packet;
use crate::states::handle_packet;

const SIZE: usize = 1_024;

pub struct Client {
    pub stream: TcpStream,
    pub ctx: Context,
}

impl Client {
    pub fn new(stream: TcpStream) -> Client {
        Client { stream, ctx: Context { state: State::Handshake } }
    }

    // FIXME: we should not crash on unknown packet for now
    // NOTE: we probably can skip buffer initialization
    // NOTE: The move should only occur after every packet is processed
    // right now we only read one packet
    pub async fn run(&mut self) {
        let mut buf = [0u8; SIZE];
        let mut len = 0;

        self.stream.set_nodelay(true).expect("set_nodelay call failed");

        while let Ok(n) = self.stream.read(&mut buf[len..SIZE]).await
            && n > 0
        {
            println!("Buffer = {:?}", &buf[len..len + n]);

            len += n;

            while let Some((pkt, processed)) = read_packet(&buf[..len], self).unwrap() {
                handle_packet(pkt, self).await;

                buf.copy_within(processed..len, 0);
                len -= processed;
            }
        }
    }
}
