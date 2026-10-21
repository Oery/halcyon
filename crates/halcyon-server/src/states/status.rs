use mc_protocol::data::motd::*;
use mc_protocol::packets::Packets;
use mc_protocol::packets::client::*;
use mc_protocol::packets::server::*;

use crate::Client;
use crate::error::Error;

fn status(online: usize) -> StatusPacket<'static> {
    StatusPacket {
        json: Status {
            version: Version { name: "halcyon-server", protocol: 777 },
            description: Description { text: "hello world!" },
            players: Players {
                max: crate::lobby::MAX_PLAYERS as i32,
                online: online as i32,
                sample: vec![],
            },
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
            let online = client.lobby.active_count();
            client.send(&status(online)).await?;
        }
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };

    Ok(())
}
