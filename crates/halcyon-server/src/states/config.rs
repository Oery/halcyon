use mc_protocol::packets::Packets;
use mc_protocol::packets::client::PlayLogin;

use crate::Client;
use crate::error::Error;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::ClientInformation(p) => {
            client.ctx.skin_parts = p.skin_parts.0;
        }
        Packets::ConfigPluginMessage(_) | Packets::ConfigKnownPacks(_) => {}
        Packets::AcknowledgeFinish(_) => {
            // Entity ID 0 is vanilla's unassigned sentinel (Entity.getId
            // throws on 0); the Default uses 1.
            client.send(&PlayLogin::default()).await?;
            super::play::enter_play(client).await?;
            client.ctx.state = mc_protocol::State::Play;
            return Ok(());
        }
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };

    Ok(())
}
