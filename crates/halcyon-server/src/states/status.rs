use mc_protocol::packets::Payload;
use mc_protocol::packets::client::*;

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

pub async fn handle_status(client: &mut Client) {
    status().write_packet(&mut client.stream).await.unwrap();
}
