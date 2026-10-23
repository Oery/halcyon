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
            client.claim_identity().await?;
            // The player keeps this entity ID in every instance it visits.
            let entity_id = client
                .server
                .entity_ids
                .next()
                .ok_or(crate::instance::AdmissionError::IdExhausted)?;
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
            let server = client.server.clone();
            client.send_encoded(&server.packets.commands).await?;
            client.ctx.state = mc_protocol::State::Play;
            // The fullest hub with room, which may be on another shard.
            return client.place(entity_id).await;
        }
        _ => return Err(Error::UnexpectedResponse),
    };

    Ok(())
}
