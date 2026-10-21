use futures_lite::{AsyncReadExt, AsyncWriteExt, future};
use glommio::{net::TcpStream, timer::Timer};
use mc_protocol::{
    State,
    decode::MAX_FRAME_LEN,
    packets::{Context, Payload, client::PlayKeepAlive},
};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    chunks::{self, ChunkTracker},
    error::Error,
    lifecycle::{Keepalive, LOGIN_TIMEOUT, LoginProgress},
    lobby::{Lobby, ObserverMetrics, Profile, Session, Transform},
    packet::read_packet,
    server::Server,
    states::handle_packet,
    transport::ClientStream,
    visibility::{Observer, WRITE_SLICE_BYTES, WRITE_SLICE_FRAMES},
};

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
    pub pending_teleport: Option<i32>,
    pub teleport_deadline: Option<Instant>,
    selected_slot: u8,
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
            pending_teleport: None,
            teleport_deadline: None,
            selected_slot: 0,
            chunks: ChunkTracker::new(now),
            packet_buffer: Vec::with_capacity(INITIAL_BUFFER),
            login_deadline: now + LOGIN_TIMEOUT,
        }
    }

    pub fn entity_id(&self) -> Result<i32, Error> {
        self.session.as_ref().map(Session::entity_id).ok_or(Error::UnexpectedResponse)
    }

    /// Reserve an authenticated human before its entity ID is sent in Play Login.
    pub async fn reserve_player(&mut self) -> Result<(), Error> {
        use mc_protocol::packets::client::PlayerInfoUpdate;
        use mc_protocol::types::ProfileProperty;

        if self.session.is_some() {
            return Err(Error::UnexpectedResponse);
        }
        let uuid = self.ctx.player_uuid.ok_or(Error::UnexpectedResponse)?;
        let username = self.ctx.username.as_deref().ok_or(Error::UnexpectedResponse)?;
        let properties = self
            .ctx
            .profile_properties
            .iter()
            .map(|p| ProfileProperty {
                name: &p.name,
                value: &p.value,
                signature: p.signature.as_deref(),
            })
            .collect();
        self.packet_buffer.clear();
        PlayerInfoUpdate { uuid, username, properties }
            .write_packet(&mut self.packet_buffer)
            .await?;
        let info: Rc<[u8]> = Rc::from(self.packet_buffer.as_slice());
        let profile = Rc::new(Profile { uuid, username: username.to_owned(), info: info.clone() });
        let (x, y, z, yaw, pitch) = self.server.lobby.spawn;
        self.pos = Transform { x, y, z, yaw, pitch, on_ground: false };
        self.proposed = self.pos;
        self.session = Some(self.lobby.register(profile, self.pos, self.ctx.skin_parts).await?);
        self.profile_info = Some(info);
        // The shared encoded profile is sufficient for subsequent player-info sends.
        self.ctx.profile_properties.clear();
        Ok(())
    }

    pub fn propose_position(&self) {
        if let Some(session) = &self.session {
            session.propose_movement(crate::movement::MovementProposal {
                transform: self.proposed,
                sneaking: self.sneaking,
                sprinting: self.sprinting,
            });
        }
    }

    pub fn reconcile_players(&mut self) -> Result<(), Error> {
        if self.pending_teleport.is_some() {
            return Ok(());
        }
        let Some(session) = &self.session else { return Ok(()) };
        let id = session.entity_id();
        self.lobby
            .with_registry(|registry| self.observer.reconcile(registry, id, self.chunks.terrain()))
            .map_err(Error::from)
    }

    async fn send_player_frame(&mut self) -> Result<Option<(usize, usize)>, Error> {
        let Some(frame) = self.observer.next_frame()? else {
            return Ok(None);
        };
        let frames = self.observer.next_frame_count();
        self.send_encoded(&frame).await?;
        self.observer.frame_sent();
        Ok(Some((frame.len(), frames)))
    }

    fn report_observer(&self) {
        if let Some(session) = &self.session {
            session.record_observer(ObserverMetrics {
                roster: self.observer.roster_count(),
                visible: self.observer.visible_count(),
                queued_bytes: self.observer.queued_bytes(),
                queued_frames: self.observer.queued_frames(),
                high_water_bytes: self.observer.high_water_bytes(),
                coalesced: self.observer.coalesced_updates(),
                sent_bytes: self.observer.sent_bytes(),
            });
        }
    }

    async fn drain_players(&mut self) -> Result<(), Error> {
        self.reconcile_players()?;
        let mut bytes = 0;
        let mut frames = 0;
        while frames < WRITE_SLICE_FRAMES {
            let Some((size, count)) = self.send_player_frame().await? else {
                break;
            };
            bytes += size;
            frames += count;
            if bytes >= WRITE_SLICE_BYTES {
                break;
            }
        }
        self.report_observer();
        if frames >= WRITE_SLICE_FRAMES || bytes >= WRITE_SLICE_BYTES {
            glommio::executor().yield_now().await;
        }
        Ok(())
    }

    /// The single connection writer commits removals before their chunk unload.
    pub async fn remove_players_before_unload(&mut self, column: (i32, i32)) -> Result<(), Error> {
        self.observer.before_unload(column)?;
        let mut frames = 0;
        while self.observer.has_unload_barrier() {
            if self.send_player_frame().await?.is_none() {
                return Err(Error::UnexpectedResponse);
            }
            frames += 1;
            if frames % 32 == 0 {
                glommio::executor().yield_now().await;
            }
        }
        self.report_observer();
        Ok(())
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
        .await?;
        self.lobby.record_sent(self.packet_buffer.len());
        Ok(())
    }

    pub async fn send_encoded(&mut self, bytes: &[u8]) -> Result<(), Error> {
        with_deadline(Instant::now() + WRITE_TIMEOUT, async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await?;
            Ok(())
        })
        .await?;
        self.lobby.record_sent(bytes.len());
        Ok(())
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
        .await?;
        self.lobby.record_sent(header.len() + data.len() + suffix.len());
        Ok(())
    }

    pub async fn send_buffered_chunk(&mut self, data: &[u8], suffix: &[u8]) -> Result<(), Error> {
        with_deadline(Instant::now() + WRITE_TIMEOUT, async {
            self.stream.write_all(&self.packet_buffer).await?;
            self.stream.write_all(data).await?;
            self.stream.write_all(suffix).await?;
            self.stream.flush().await?;
            Ok(())
        })
        .await?;
        self.lobby.record_sent(self.packet_buffer.len() + data.len() + suffix.len());
        Ok(())
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

    async fn apply_player_update(
        &mut self,
        update: crate::player::PlayerUpdate,
    ) -> Result<(), Error> {
        use crate::lifecycle::{DamageKind, TELEPORT_TIMEOUT};
        use mc_protocol::packets::client::{
            CombatKill, DamageEvent, PlayerAbilities, Respawn, SetContainerContent,
            SetContainerSlot, SetCursorItem, SetHealth, SetHeldItemResponse, SyncPlayerPosition,
        };
        use mc_protocol::types::Nbt;
        if update.respawn {
            self.send(&Respawn {
                dimension_type: 0,
                dimension_name: "minecraft:overworld",
                hashed_seed: 0,
                game_mode: 0,
                previous_game_mode: 0,
                is_debug: false,
                is_flat: false,
                death_location: None,
                portal_cooldown: 0,
                sea_level: 63,
                data_kept: 0,
            })
            .await?;
            self.send(&PlayerAbilities { flags: 0, flying_speed: 0.05, fov_modifier: 0.1 }).await?;
        }
        if let Some(health) = update.health {
            self.send(&SetHealth { health, food: 20, saturation: 5.0 }).await?;
        }
        if let Some(damage) = update.damage {
            let source = damage.source.and_then(|id| id.checked_add(1)).unwrap_or(0);
            self.send(&DamageEvent {
                entity_id: self.entity_id()?,
                source_type_id: damage.kind.protocol_id(),
                source_cause_id: source,
                source_direct_id: source,
                source_position: None,
            })
            .await?;
        }
        if let Some(damage) = update.death {
            let message = match damage.kind {
                DamageKind::Fall => "You fell from a high place",
                DamageKind::Void => "You fell out of the world",
                DamageKind::Game => "You died",
            };
            self.send(&CombatKill { player_id: self.entity_id()?, message: Nbt::Str(message) })
                .await?;
        }
        if let Some(inventory) = update.inventory {
            if update.inventory_full {
                self.send(&SetContainerContent {
                    window_id: 0,
                    state_id: inventory.state_id,
                    slots: &inventory.slots,
                    carried: &inventory.cursor,
                })
                .await?;
            } else {
                for (slot, stack) in inventory.slots.iter().enumerate() {
                    if update.inventory_slots & (1u64 << slot) != 0 {
                        self.send(&SetContainerSlot {
                            window_id: 0,
                            state_id: inventory.state_id,
                            slot: slot as i16,
                            stack,
                        })
                        .await?;
                    }
                }
            }
            self.send(&SetCursorItem { stack: &inventory.cursor }).await?;
            if update.inventory_full || self.selected_slot != inventory.selected {
                self.send(&SetHeldItemResponse { slot: i32::from(inventory.selected) }).await?;
                self.selected_slot = inventory.selected;
            }
        }
        if let Some(teleport) = update.teleport {
            let p = teleport.destination;
            self.pos = p;
            self.proposed = p;
            self.sneaking = false;
            self.sprinting = false;
            self.chunks.reconcile(&self.server.lobby, p.chunk());
            self.send(&SyncPlayerPosition {
                teleport_id: teleport.id,
                x: p.x,
                y: p.y,
                z: p.z,
                vel_x: 0.0,
                vel_y: 0.0,
                vel_z: 0.0,
                yaw: p.yaw,
                pitch: p.pitch,
                flags: 0,
            })
            .await?;
            self.pending_teleport = Some(teleport.id);
            self.teleport_deadline = Some(Instant::now() + TELEPORT_TIMEOUT);
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
            if self.observer.queued_frames() > 0 {
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
