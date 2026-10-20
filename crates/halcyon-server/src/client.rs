use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite};
use glommio::net::TcpStream;
use mc_protocol::packets::Context;
use openssl::symm::Crypter;

use crate::packet::read_packet;
use crate::server::Server;
use crate::states::handle_packet;

const SIZE: usize = 1_024;

pub struct Client {
    pub stream: ClientStream,
    pub ctx: Context,
    pub server: Arc<Server>,
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
    ) -> Poll<std::io::Result<usize>> {
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
                        let m =
                            dec.update(&encrypted[..n], &mut plain).expect("AES decrypt failed");
                        buf[..m].copy_from_slice(&plain[..m]);
                        Poll::Ready(Ok(m))
                    }
                    Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                    Poll::Pending => Poll::Pending,
                }
            }
            ClientStream::Placeholder => unreachable!("polled placeholder stream"),
        }
    }
}

impl AsyncWrite for ClientStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();

        match this {
            ClientStream::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            ClientStream::Encrypted { stream, enc, .. } => {
                let mut encrypted = vec![0u8; buf.len() + 16];
                let n = enc.update(buf, &mut encrypted).expect("AES encrypt failed");
                // NOTE: must loop until all `n` bytes flush; single poll_write may short-write.
                let mut written = 0;
                while written < n {
                    match Pin::new(&mut *stream).poll_write(cx, &encrypted[written..n]) {
                        Poll::Ready(Ok(0)) => {
                            return Poll::Ready(Err(std::io::Error::from(
                                std::io::ErrorKind::WriteZero,
                            )));
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
            ClientStream::Placeholder => unreachable!("polled placeholder stream"),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(stream) => Pin::new(stream).poll_flush(cx),
            ClientStream::Encrypted { stream, .. } => Pin::new(stream).poll_flush(cx),
            ClientStream::Placeholder => unreachable!("polled placeholder stream"),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(stream) => Pin::new(stream).poll_close(cx),
            ClientStream::Encrypted { stream, .. } => Pin::new(stream).poll_close(cx),
            ClientStream::Placeholder => unreachable!("polled placeholder stream"),
        }
    }
}

impl Client {
    pub fn new(stream: TcpStream, server: Arc<Server>) -> Client {
        Client { stream: ClientStream::Plain(stream), ctx: Context::default(), server }
    }

    pub fn enable_encryption(&mut self, enc: Crypter, dec: Crypter) {
        let plain = std::mem::replace(&mut self.stream, ClientStream::Placeholder);
        let ClientStream::Plain(raw) = plain else {
            unreachable!("encryption enabled twice")
        };

        self.stream = ClientStream::Encrypted { stream: raw, enc, dec };
    }

    pub async fn run(&mut self) {
        let mut buf = [0u8; SIZE];
        let mut len = 0;

        match &self.stream {
            ClientStream::Plain(stream) => {
                stream.set_nodelay(true).expect("set_nodelay call failed")
            }
            ClientStream::Encrypted { stream, .. } => {
                stream.set_nodelay(true).expect("set_nodelay call failed")
            }
            ClientStream::Placeholder => unreachable!("placeholder at run start"),
        };
        while let Ok(n) = self.stream.read(&mut buf[len..SIZE]).await
            && n > 0
        {
            println!("Buffer = {:?}", &buf[len..len + n]);

            len += n;

            let mut total_read = 0;

            loop {
                match read_packet(&buf[total_read..len], self) {
                    Ok(Some((pkt, processed))) => {
                        if let Some(pkt) = pkt {
                            handle_packet(pkt, self).await;
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
        }
    }
}
