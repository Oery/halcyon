use mc_protocol::data::motd::*;
use mc_protocol::packets::Packets;
use mc_protocol::packets::client::*;
use mc_protocol::packets::server::*;

use crate::Client;
use crate::error::Error;

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

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::PingRequest(PingRequest { time }) => {
            client.send(&PingResponse { time }).await?;
        }
        Packets::StatusRequest(_) => {
            client.send(&status()).await?;
        }
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };

    Ok(())
}
