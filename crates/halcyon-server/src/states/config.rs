use mc_protocol::packets::Packets;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::PlayLogin;

use crate::Client;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    match pkt {
        Packets::ClientInformation(_)
        | Packets::ConfigPluginMessage(_)
        | Packets::ConfigKnownPacks(_) => {}
        Packets::AcknowledgeFinish(_) => {
            // Entity ID 0 is vanilla's unassigned sentinel (Entity.getId
            // throws on 0); the Default uses 1.
            PlayLogin::default().write_packet(&mut client.stream).await.unwrap();
            super::play::enter_play(client).await;
            client.ctx.state = mc_protocol::State::Play;
        }
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}
