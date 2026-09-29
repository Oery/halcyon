use mc_protocol::packets::Packets;

use crate::Client;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    match pkt {
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}
