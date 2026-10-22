use futures_lite::{AsyncReadExt, future};
use glommio::{net::TcpStream, timer::Timer};
use mc_protocol::{
    State,
    decode::MAX_FRAME_LEN,
    packets::{Context, client::PlayKeepAlive},
};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    chunks::{self, ChunkTracker},
    error::Error,
    inventory::HotbarSlot,
    lifecycle::{Keepalive, LOGIN_TIMEOUT, LoginProgress},
    lobby::{Lobby, Session, Transform},
    packet::read_packet,
    server::Server,
    states::handle_packet,
    transport::ClientStream,
    visibility::Observer,
};

mod gameplay;
mod publication;
mod writer;

const INITIAL_BUFFER: usize = 1_024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Client {
    pub stream: ClientStream<TcpStream>,
    pub ctx: Context,
    pub server: Arc<Server>,
    pub login: LoginProgress,
    pub keepalive: Keepalive,
    pub pos: Transform,
    pub proposed: Transform,
    pub sneaking: bool,
    pub sprinting: bool,
    pub lobby: Lobby,
    pub session: Option<Session>,
    pub profile_info: Option<Rc<[u8]>>,
    observer: Observer,
    world_observer: crate::world_sync::WorldObserver,
    pub pending_teleport: Option<i32>,
    pub teleport_deadline: Option<Instant>,
    selected_slot: HotbarSlot,
    pub chunks: ChunkTracker,
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
    pub fn new(stream: TcpStream, server: Arc<Server>, lobby: Lobby) -> Self {
        let now = Instant::now();
        Self {
            stream: ClientStream::new(stream),
            ctx: Context::default(),
            server,
            login: LoginProgress::default(),
            keepalive: Keepalive::new(now),
            pos: Transform::default(),
            proposed: Transform::default(),
            sneaking: false,
            sprinting: false,
            lobby,
            session: None,
            profile_info: None,
            observer: Observer::default(),
            world_observer: crate::world_sync::WorldObserver::default(),
            pending_teleport: None,
            teleport_deadline: None,
            selected_slot: HotbarSlot::FIRST,
            chunks: ChunkTracker::new(now),
            packet_buffer: Vec::with_capacity(INITIAL_BUFFER),
            login_deadline: now + LOGIN_TIMEOUT,
        }
    }

    fn deadline(&self) -> Instant {
        if self.ctx.state == State::Play {
            self.teleport_deadline.map_or(self.keepalive.deadline(), |deadline| {
                deadline.min(self.keepalive.deadline())
            })
        } else {
            self.login_deadline
        }
    }

    fn operation_deadline(&self) -> Instant {
        if self.ctx.state == State::Play {
            let write = self
                .keepalive
                .response_deadline()
                .unwrap_or_else(|| Instant::now() + WRITE_TIMEOUT);
            self.teleport_deadline.map_or(write, |deadline| deadline.min(write))
        } else {
            self.login_deadline
        }
    }

    async fn maintain(&mut self) -> Result<(), Error> {
        let now = Instant::now();
        if self.ctx.state == State::Play {
            if let Some(session) = &self.session {
                let accepted = session.accepted_transform();
                let previous = self.pos;
                self.pos = accepted;
                if previous.chunk() != accepted.chunk() {
                    self.chunks.reconcile(&self.server.lobby, accepted.chunk());
                }
                let update = session.take_update();
                self.apply_player_update(update).await?;
            }
            if self.teleport_deadline.is_some_and(|deadline| now >= deadline) {
                return Err(Error::Timeout);
            }
            if let Some(id) = self.keepalive.tick(now)? {
                self.send(&PlayKeepAlive { id }).await?;
            }
            let deadline = self.operation_deadline();
            with_deadline(deadline, self.drain_players()).await?;
            with_deadline(deadline, chunks::send_pending(self)).await?;
        } else if now >= self.login_deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    pub async fn run(&mut self) {
        let result = self.run_connection().await;
        // Release the reservation even when callers retain the Client after run.
        self.session.take();
        if matches!(result, Err(Error::PlayerBacklog)) {
            self.lobby.record_slow_client();
        }
        if let Err(error) = result {
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
            let mut wake = self.deadline();
            if self.ctx.state == State::Play
                && let Some(deadline) = self.chunks.deadline()
            {
                wake = wake.min(deadline);
            }
            if self.observer.has_pending() || self.world_observer.pending_work() {
                wake = wake.min(Instant::now());
            }
            let read = future::or(
                async { Some(self.stream.read(&mut buf[len..]).await) },
                future::or(
                    async {
                        Timer::new(wake.saturating_duration_since(Instant::now())).await;
                        None
                    },
                    async {
                        if let Some(session) = &self.session {
                            session.wait_changed().await;
                        } else {
                            future::pending::<()>().await;
                        }
                        None
                    },
                ),
            )
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
