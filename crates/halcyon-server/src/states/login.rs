use mc_protocol::packets::Packets;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::*;
use mc_protocol::packets::server::*;

use crate::Client;
use crate::encryption::generate_verify_token;

// NOTE: we should probably separate client into different states
// only an unauthenticated client needs the verify token
// while authenticated client will need to store more data

// TODO: We need to send a request to Mojang API

async fn handle_login_start(p: LoginStart<'_>, client: &mut Client) {
    let verify_token = generate_verify_token().unwrap();

    let request = EncryptionRequest {
        server_id: "halcyon",
        public_key: &client.server.keys.public,
        verify_token: &verify_token,
        should_authenticate: true,
    };

    request.write_packet(&mut client.stream).await.unwrap();

    client.ctx.verify_token = Some(verify_token);
}

pub async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    use Packets::*;

    match pkt {
        LoginStart(p) => handle_login_start(p, client).await,
        SetCompression(c) => client.ctx.compression = Some(c.threshold),
        _ => eprintln!("unhandled packet: {pkt:?}"),
    };
}
