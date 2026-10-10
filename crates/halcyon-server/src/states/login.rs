use mc_protocol::packets::Packets;

use crate::Client;

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    use Packets::*;

    match pkt {
        SetCompression(c) => client.ctx.compression = Some(c.threshold),
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}
