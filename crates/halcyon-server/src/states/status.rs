use mc_protocol::packets::client::*;
use mc_protocol::packets::{Packets, Payload};

use crate::Client;

fn status() -> StatusPacket<'static> {
    StatusPacket {
        json: Status {
            version: Version { name: "halcyon-server", protocol: 777 },
            description: Description { text: "hello world!" },
            players: Players { max: 20_000, online: 10, sample: vec![] },
            enforces_secure_chat: false,
            favicon: None,
        },
    }
}

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    let w = &mut client.stream;

    match pkt {
        Packets::PingRequest(Ping { time }) => {
            Ping { time }.write_packet(w).await.unwrap();
        }
        Packets::StatusRequest(_) => {
            status().write_packet(w).await.unwrap();
        }
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}
