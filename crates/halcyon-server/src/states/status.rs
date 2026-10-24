use mc_protocol::data::motd::*;
use mc_protocol::packets::Packets;
use mc_protocol::packets::client::*;
use mc_protocol::packets::server::*;

use crate::Client;
use crate::error::Error;

fn status(motd: &str, online: usize) -> StatusPacket<'_> {
    StatusPacket {
        json: Status {
            version: Version { name: "halcyon-server", protocol: 777 },
            description: Description { text: motd },
            players: Players {
                // The lobby has no player cap; always advertise room for one more.
                max: online.saturating_add(1) as i32,
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
            let online = client.server.directory.online();
            let server = client.server.clone();
            client.send(&status(&server.motd, online)).await?;
        }
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };

    Ok(())
}
