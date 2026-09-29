use futures_lite::{AsyncReadExt, stream::StreamExt};
use glommio::net::{TcpListener, TcpStream};

use mc_protocol::decode::take;
use mc_protocol::error::DecodeError;
use mc_protocol::packets::Packets;
use mc_protocol::state::State;
use mc_protocol::{PacketWriter, VarInt};

use mc_protocol::packets::Context;
use mc_protocol::packets::Payload;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
static ADDR: &str = "127.0.0.1:25565";

const SIZE: usize = 1_024;

pub struct Client {
    stream: TcpStream,
    ctx: Context,
}

impl Client {
    fn new(stream: TcpStream) -> Client {
        Client { stream, ctx: Context { state: State::HANDSHAKE } }
    }
}

use mc_protocol::packets::client::*;

fn status() -> Status<'static> {
    Status {
        version: Version { name: "halcyon-server", protocol: 777 },
        description: Description { text: "hello world!" },
        players: Players { max: 20_000, online: 10, sample: vec![] },
        enforces_secure_chat: false,
        favicon: None,
    }
}

// TODO: send back a status response
// TODO: reject invalid next states
async fn handle_packet(pkt: Packets<'_>, client: &mut Client) {
    dbg!(&pkt);

    match pkt {
        Packets::ServerListPing(slp) => {
            client.ctx.state = slp.next_state;
        }
        Packets::PingRequest(Ping { time }) => {
            Ping { time }.write_packet(&mut client.stream).await.unwrap();
        }
        Packets::StatusRequest(_) => {
            status().write_packet(&mut client.stream).await.unwrap();
        }
    };
}

fn try_read_varint(buf: &mut &[u8]) -> Result<Option<usize>, DecodeError> {
    let mut remaining = *buf;

    match VarInt::read(&mut remaining) {
        Ok(VarInt(value)) => {
            let value = usize::try_from(value).map_err(|_| DecodeError::InvalidVarInt)?;
            *buf = remaining;
            Ok(Some(value))
        }
        Err(DecodeError::UnexpectedEof) => Ok(None),
        Err(err) => Err(err),
    }
}

type DecodeResult<T> = Result<T, DecodeError>;

fn read_packet<'p>(buf: &'p [u8], c: &mut Client) -> DecodeResult<Option<(Packets<'p>, usize)>> {
    let mut remaining = buf;

    let Some(frame_len) = try_read_varint(&mut remaining)? else {
        return Ok(None);
    };

    if remaining.len() < frame_len {
        return Ok(None);
    }

    let mut frame = take(&mut remaining, frame_len)?;
    let pkt = Packets::read(&mut frame, &c.ctx)?;

    Ok(Some((pkt, buf.len() - remaining.len())))
}

// FIXME: we should not crash on unknown packet for now
// NOTE: we probably can skip buffer initialization
// NOTE: The move should only occur after every packet is processed
// right now we only read one packet
#[rustfmt::skip]
async fn handle_client(stream: TcpStream) {
    let mut buf = [0u8; SIZE];
    let mut len = 0;

    let mut c = Client::new(stream);

    c.stream.set_nodelay(true).expect("set_nodelay call failed");

    while let Ok(n) = c.stream.read(&mut buf[len..SIZE]).await && n > 0 {
        println!("Buffer = {:?}", &buf[len..len + n]);

        len += n;

        while let Some((pkt, processed)) = read_packet(&buf[..len], &mut c).unwrap() {
            handle_packet(pkt, &mut c).await;

            buf.copy_within(processed..len, 0);
            len -= processed;
        }
    }
}

#[glommio::main(placement = Fixed(0))]
async fn main() {
    glommio::spawn_local(async {
        let listener = TcpListener::bind(ADDR).unwrap();
        let mut incoming = listener.incoming();

        println!("Listening on {}", listener.local_addr().unwrap());

        while let Some(Ok(conn)) = incoming.next().await {
            println!("connection!");

            glommio::spawn_local(async move {
                handle_client(conn).await;
            })
            .detach();
        }

        eprintln!("error: could not accept client connection");
    })
    .await;
}
