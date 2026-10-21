use mc_protocol::packets::Packets;

use crate::Client;
use crate::error::Error;

// 1 for Status, 2 for Login, 3 for Transfer. Intents 2 and 3 both transition
// to the Login state, but 3 indicates that the client is connecting due to
// a Transfer packet received from another server.
// If the server is not expecting transfers, it may choose to reject
// the connection by replying with a Disconnect (login) packet.

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) -> Result<(), Error> {
    match pkt {
        Packets::ServerListPing(slp) => client.ctx.state = slp.next_state.state(),
        _ => return Err(Error::UnexpectedResponse),
    };

    Ok(())
}
