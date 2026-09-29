mod config;
mod handshake;
mod login;
mod play;
mod status;

use mc_protocol::packets::{InnerPacket, Packets};
use mc_protocol::state::State;

use crate::Client;

// TODO: send back a status response
pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    dbg!(&pkt);

    match pkt.state() {
        State::Config => config::handle_packet(pkt, client).await,
        State::Handshake => handshake::handle_packet(pkt, client).await,
        State::Login => login::handle_packet(pkt, client).await,
        State::Status => status::handle_packet(pkt, client).await,
        State::Play => play::handle_packet(pkt, client).await,
    };
}
