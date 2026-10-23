mod config;
mod handshake;
mod login;
pub mod play;
mod status;

use mc_protocol::packets::Packets;
use mc_protocol::state::State;

use crate::Client;
use crate::error::Error;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    // Shared payload types (ClientInformation) occur in more than one state.
    // The connection state, already used by the decoder, owns dispatch.
    match client.ctx.state {
        State::Config => config::handle_packet(pkt, client).await,
        State::Handshake => handshake::handle_packet(pkt, client).await,
        State::Login => login::handle_packet(pkt, client).await,
        State::Status => status::handle_packet(pkt, client).await,
        State::Play => play::handle_packet(pkt, client).await,
    }
}
