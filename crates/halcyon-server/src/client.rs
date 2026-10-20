use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite};
use glommio::net::TcpStream;
use mc_protocol::packets::{Context, Payload};
use openssl::symm::Crypter;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::packet::read_packet;
use crate::server::Server;
use crate::states::handle_packet;

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
    pub stream: ClientStream,
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
}

pub enum ClientStream {
    Plain(TcpStream),
    Encrypted { stream: TcpStream, enc: Crypter, dec: Crypter },
    Placeholder,
}

impl AsyncRead for ClientStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        match this {
            ClientStream::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            ClientStream::Encrypted { stream, dec, .. } => {
                // NOTE: decrypt in place after the read; CFB8 is 1:1 streaming.
                let mut encrypted = vec![0u8; buf.len()];
                match Pin::new(stream).poll_read(cx, &mut encrypted) {
                    Poll::Ready(Ok(0)) => Poll::Ready(Ok(0)),
                    Poll::Ready(Ok(n)) => {
                        let mut plain = vec![0u8; n + 16];
                        match dec.update(&encrypted[..n], &mut plain) {
                            Ok(m) => {
                                buf[..m].copy_from_slice(&plain[..m]);
                                Poll::Ready(Ok(m))
                            }
                            // Crypto failure mid-stream: unrecoverable, drop the
                            // connection rather than delivering corrupt bytes.
                            Err(_) => Poll::Ready(Err(io::Error::from(io::ErrorKind::InvalidData))),
                        }
                    }
                    Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                    Poll::Pending => Poll::Pending,
                }
            }
            ClientStream::Placeholder => {
                Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
            }
        }
    }
}

impl AsyncWrite for ClientStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        match this {
            ClientStream::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            ClientStream::Encrypted { stream, enc, .. } => {
                let mut encrypted = vec![0u8; buf.len() + 16];
                let n = match enc.update(buf, &mut encrypted) {
                    Ok(n) => n,
                    Err(_) => return Poll::Ready(Err(io::Error::from(io::ErrorKind::InvalidData))),
                };
                // NOTE: must loop until all `n` bytes flush; single poll_write may short-write.
                let mut written = 0;
                while written < n {
                    match Pin::new(&mut *stream).poll_write(cx, &encrypted[written..n]) {
                        Poll::Ready(Ok(0)) => {
                            return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero)));
                        }
                        Poll::Ready(Ok(m)) => written += m,
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending if written == 0 => return Poll::Pending,
                        // Partial write then Pending: keep polling; do not lose `written`.
                        Poll::Pending => continue,
                    }
                }
                Poll::Ready(Ok(buf.len()))
            }
            ClientStream::Placeholder => {
                Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(stream) => Pin::new(stream).poll_flush(cx),
            ClientStream::Encrypted { stream, .. } => Pin::new(stream).poll_flush(cx),
            ClientStream::Placeholder => {
                Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
            }
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(stream) => Pin::new(stream).poll_close(cx),
            ClientStream::Encrypted { stream, .. } => Pin::new(stream).poll_close(cx),
            ClientStream::Placeholder => {
                Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
            }
        }
    }
}

impl Client {
    pub fn new(stream: TcpStream, server: Arc<Server>) -> Client {
        Client {
            stream: ClientStream::Plain(stream),
            ctx: Context::default(),
            server,
            keepalive_id: 0,
            last_keepalive: Instant::now(),
            pos: PlayerPosition::default(),
            pending_teleport: None,
            next_teleport_id: 0,
            sent_chunks: std::collections::HashSet::new(),
            view_distance: 4,
        }
    }
    /// Sends one packet; any write failure ends the connection at the caller.
    pub async fn send<'p, P: Payload<'p>>(&mut self, pkt: &P) -> Result<(), Error> {
        Ok(pkt.write_packet(&mut self.stream).await?)
    }

    /// Enables encryption exactly once (login flow). A second call is a
    /// server bug: logged, connection kept on the existing stream.
    pub fn enable_encryption(&mut self, enc: Crypter, dec: Crypter) {
        let plain = std::mem::replace(&mut self.stream, ClientStream::Placeholder);
        let ClientStream::Plain(raw) = plain else {
            eprintln!("encryption enabled twice; keeping existing stream");
            self.stream = plain;
            return;
        };

        self.stream = ClientStream::Encrypted { stream: raw, enc, dec };
    }

    pub async fn run(&mut self) {
        let mut buf = [0u8; SIZE];
        let mut len = 0;

        let nodelay = match &self.stream {
            ClientStream::Plain(stream) => stream.set_nodelay(true),
            ClientStream::Encrypted { stream, .. } => stream.set_nodelay(true),
            ClientStream::Placeholder => return,
        };
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
