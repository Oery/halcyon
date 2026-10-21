use futures_lite::{AsyncReadExt, AsyncWriteExt};
use glommio::net::TcpStream;
use mc_protocol::packets::{Context, Payload};
use openssl::symm::Crypter;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::packet::read_packet;
use crate::server::Server;
use crate::states::handle_packet;
use crate::transport::ClientStream;

const SIZE: usize = 1_024;

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
    pub keepalive_id: i64,
    pub last_keepalive: Instant,
    pub pos: PlayerPosition,
    pub pending_teleport: Option<i32>,
    pub next_teleport_id: i32,
    /// Chunks the client has loaded: sent in a `ChunkData` batch and not
    /// yet unloaded. Updated on entry burst and on chunk-border crossings.
    pub sent_chunks: std::collections::HashSet<(i32, i32)>,
    /// Chebyshev radius of chunks sent around the player.
    pub view_distance: i32,
    pub packet_buffer: Vec<u8>,
}

impl Client {
    pub fn new(stream: TcpStream, server: Arc<Server>) -> Client {
        Client {
            stream: ClientStream::new(stream),
            ctx: Context::default(),
            server,
            keepalive_id: 0,
            last_keepalive: Instant::now(),
            pos: PlayerPosition::default(),
            pending_teleport: None,
            next_teleport_id: 0,
            sent_chunks: std::collections::HashSet::new(),
            view_distance: 32,
            packet_buffer: Vec::with_capacity(SIZE),
        }
    }
    /// Encode into reusable storage before touching the socket.
    pub async fn send<'p, P: Payload<'p>>(&mut self, pkt: &P) -> Result<(), Error> {
        self.packet_buffer.clear();
        pkt.write_packet(&mut self.packet_buffer).await?;
        self.send_encoded_buffer().await
    }

    pub async fn send_encoded_buffer(&mut self) -> Result<(), Error> {
        self.stream.write_all(&self.packet_buffer).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn send_encoded(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn send_chunk_parts(
        &mut self,
        header: &[u8],
        data: &[u8],
        suffix: &[u8],
    ) -> Result<(), Error> {
        self.stream.write_all(header).await?;
        self.stream.write_all(data).await?;
        self.stream.write_all(suffix).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn send_buffered_chunk(&mut self, data: &[u8], suffix: &[u8]) -> Result<(), Error> {
        self.stream.write_all(&self.packet_buffer).await?;
        self.stream.write_all(data).await?;
        self.stream.write_all(suffix).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub fn enable_encryption(&mut self, enc: Crypter, dec: Crypter) -> Result<(), Error> {
        Ok(self.stream.enable_encryption(enc, dec)?)
    }

    pub async fn run(&mut self) {
        let mut buf = [0u8; SIZE];
        let mut len = 0;

        let nodelay = self.stream.inner.set_nodelay(true);
        if let Err(e) = nodelay {
            eprintln!("set_nodelay failed: {e}");
            return;
        }
        while let Ok(n) = self.stream.read(&mut buf[len..SIZE]).await
            && n > 0
        {
            len += n;

            let mut total_read = 0;

            loop {
                match read_packet(&buf[total_read..len], self) {
                    Ok(Some((pkt, processed))) => {
                        if let Some(pkt) = pkt
                            && let Err(e) = handle_packet(pkt, self).await
                        {
                            eprintln!("dropping connection: {e}");
                            return;
                        }
                        total_read += processed;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("dropping connection: {e}");
                        return;
                    }
                }
            }

            buf.copy_within(total_read..len, 0);
            len -= total_read;

            if self.ctx.state == mc_protocol::State::Play
                && self.last_keepalive.elapsed() >= Duration::from_secs(15)
            {
                use mc_protocol::packets::client::PlayKeepAlive;

                self.keepalive_id += 1;
                if let Err(e) = self.send(&PlayKeepAlive { id: self.keepalive_id }).await {
                    eprintln!("dropping connection: {e}");
                    return;
                }
                self.last_keepalive = Instant::now();
            }
        }
    }
}
