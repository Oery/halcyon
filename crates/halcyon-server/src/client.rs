use futures_lite::{AsyncReadExt, AsyncWriteExt, future};
use glommio::{net::TcpStream, timer::Timer};
use mc_protocol::{
    State,
    decode::MAX_FRAME_LEN,
    packets::{Context, Payload, client::PlayKeepAlive},
};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    error::Error,
    lifecycle::{Keepalive, LOGIN_TIMEOUT, LoginProgress},
    packet::read_packet,
    server::Server,
    states::handle_packet,
    transport::ClientStream,
};

const INITIAL_BUFFER: usize = 1_024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct PlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Default for PlayerPosition {
    fn default() -> Self {
        Self { x: 8.5, y: 64.0, z: 8.5, yaw: 0.0, pitch: 0.0, on_ground: false }
    }
}

pub struct Client {
    pub stream: ClientStream<TcpStream>,
    pub ctx: Context,
    pub server: Arc<Server>,
    pub login: LoginProgress,
    pub keepalive: Keepalive,
    pub pos: PlayerPosition,
    pub pending_teleport: Option<i32>,
    pub next_teleport_id: i32,
    pub sent_chunks: std::collections::HashSet<(i32, i32)>,
    pub view_distance: i32,
    pub packet_buffer: Vec<u8>,
    login_deadline: Instant,
}

pub async fn with_deadline<T>(
    deadline: Instant,
    work: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    if Instant::now() >= deadline {
        return Err(Error::Timeout);
    }
    future::or(work, async {
        Timer::new(deadline.saturating_duration_since(Instant::now())).await;
        Err(Error::Timeout)
    })
    .await
}

impl Client {
    pub fn new(stream: TcpStream, server: Arc<Server>) -> Self {
        let now = Instant::now();
        Self {
            stream: ClientStream::new(stream),
            ctx: Context::default(),
            server,
            login: LoginProgress::default(),
            keepalive: Keepalive::new(now),
            pos: PlayerPosition::default(),
            pending_teleport: None,
            next_teleport_id: 0,
            sent_chunks: std::collections::HashSet::new(),
            view_distance: 32,
            packet_buffer: Vec::with_capacity(INITIAL_BUFFER),
            login_deadline: now + LOGIN_TIMEOUT,
        }
    }

    pub async fn send<'p, P: Payload<'p>>(&mut self, packet: &P) -> Result<(), Error> {
        self.packet_buffer.clear();
        packet.write_packet(&mut self.packet_buffer).await?;
        self.send_encoded_buffer().await
    }

    pub async fn send_encoded_buffer(&mut self) -> Result<(), Error> {
        with_deadline(Instant::now() + WRITE_TIMEOUT, async {
            self.stream.write_all(&self.packet_buffer).await?;
            self.stream.flush().await?;
            Ok(())
        })
        .await
    }

    pub async fn send_encoded(&mut self, bytes: &[u8]) -> Result<(), Error> {
        with_deadline(Instant::now() + WRITE_TIMEOUT, async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await?;
            Ok(())
        })
        .await
    }

    pub async fn send_chunk_parts(
        &mut self,
        header: &[u8],
        data: &[u8],
        suffix: &[u8],
    ) -> Result<(), Error> {
        with_deadline(Instant::now() + WRITE_TIMEOUT, async {
            self.stream.write_all(header).await?;
            self.stream.write_all(data).await?;
            self.stream.write_all(suffix).await?;
            self.stream.flush().await?;
            Ok(())
        })
        .await
    }

    pub async fn send_buffered_chunk(&mut self, data: &[u8], suffix: &[u8]) -> Result<(), Error> {
        with_deadline(Instant::now() + WRITE_TIMEOUT, async {
            self.stream.write_all(&self.packet_buffer).await?;
            self.stream.write_all(data).await?;
            self.stream.write_all(suffix).await?;
            self.stream.flush().await?;
            Ok(())
        })
        .await
    }

    fn deadline(&self) -> Instant {
        if self.ctx.state == State::Play {
            self.keepalive.deadline()
        } else {
            self.login_deadline
        }
    }

    fn operation_deadline(&self) -> Instant {
        if self.ctx.state == State::Play {
            self.keepalive.response_deadline().unwrap_or_else(|| Instant::now() + WRITE_TIMEOUT)
        } else {
            self.login_deadline
        }
    }

    async fn maintain(&mut self) -> Result<(), Error> {
        let now = Instant::now();
        if self.ctx.state == State::Play {
            if let Some(id) = self.keepalive.tick(now)? {
                self.send(&PlayKeepAlive { id }).await?;
            }
        } else if now >= self.login_deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    pub async fn run(&mut self) {
        if let Err(error) = self.run_connection().await {
            // Routine protocol errors and disconnects must not become a
            // synchronous logging path controlled by arbitrary clients.
            if self.server.diagnostics {
                eprintln!("dropping connection: {error}");
            }
        }
    }

    async fn run_connection(&mut self) -> Result<(), Error> {
        self.stream.inner.set_nodelay(true).map_err(std::io::Error::other)?;
        let mut buf = vec![0; INITIAL_BUFFER];
        let mut len = 0;
        loop {
            self.maintain().await?;
            let mut consumed = 0;
            let mut packets = 0;
            while packets < 64 {
                let Some((packet, used)) = read_packet(&buf[consumed..len], &self.ctx)? else {
                    break;
                };
                if let Some(packet) = packet {
                    with_deadline(self.operation_deadline(), handle_packet(packet, self)).await?;
                }
                consumed += used;
                packets += 1;
            }
            buf.copy_within(consumed..len, 0);
            len -= consumed;
            if packets == 64 {
                glommio::executor().yield_now().await;
                continue;
            }
            if len == buf.len() {
                let size = (buf.len() * 2).min(MAX_FRAME_LEN + 3);
                if size == buf.len() {
                    return Err(mc_protocol::error::DecodeError::InvalidLength.into());
                }
                buf.resize(size, 0);
            }
            let wake = self.deadline();
            let read = future::or(async { Some(self.stream.read(&mut buf[len..]).await) }, async {
                Timer::new(wake.saturating_duration_since(Instant::now())).await;
                None
            })
            .await;
            match read {
                Some(Ok(0)) => return Ok(()),
                Some(Ok(n)) => len += n,
                Some(Err(error)) => return Err(error.into()),
                None => {}
            }
        }
    }
}
