use mc_protocol::packets::Packets;
use mc_protocol::packets::client::PlayLogin;

use crate::Client;
use crate::error::Error;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::ClientInformation(p) => {
            client.ctx.skin_parts = p.skin_parts.0;
            client.chunks.set_view_distance(p.render_distance);
        }
        Packets::ConfigPluginMessage(_) | Packets::ConfigKnownPacks(_) => {}
        Packets::AcknowledgeFinish(_) => {
            if client.login != crate::lifecycle::LoginProgress::Complete {
                return Err(Error::UnexpectedResponse);
            }
            // Reserve a unique entity ID before the client creates its local player.
            client.reserve_player().await?;
            let entity_id = client.entity_id()?;
            client
                .send(&PlayLogin {
                    entity_id,
                    // Advertise the server cap so later client settings can
                    // raise their effective radius without stale login limits.
                    view_distance: crate::chunks::MAX_VIEW_DISTANCE,
                    simulation_distance: crate::chunks::MAX_VIEW_DISTANCE,
                    ..PlayLogin::default()
                })
                .await?;
            super::play::enter_play(client).await?;
            client.ctx.state = mc_protocol::State::Play;
            return Ok(());
        }
        _ => return Err(Error::UnexpectedResponse),
    };

    Ok(())
}
