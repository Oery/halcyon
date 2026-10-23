use futures_lite::{AsyncRead, AsyncWrite};
use openssl::symm::Crypter;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

const WRITE_SIZE: usize = 16 * 1024;

/// Ciphertext belongs to the stream until flushed, including across Pending
/// and cancellation of a caller's write future. Each accepted byte is encrypted once.
pub struct ClientStream<S> {
    pub inner: S,
    crypto: Option<Crypto>,
}

struct Crypto {
    enc: Crypter,
    dec: Crypter,
    read_scratch: Vec<u8>,
    pending: Vec<u8>,
    written: usize,
}

impl<S> ClientStream<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, crypto: None }
    }

    /// Moves the cipher state onto another socket handle for the same
    /// connection. Callers flush first; every send already ends flushed.
    pub fn map_inner<T>(self, map: impl FnOnce(S) -> T) -> ClientStream<T> {
        debug_assert!(self.crypto.as_ref().is_none_or(|crypto| crypto.pending.is_empty()));
        ClientStream { inner: map(self.inner), crypto: self.crypto }
    }

    pub fn enable_encryption(&mut self, enc: Crypter, dec: Crypter) -> io::Result<()> {
        if self.crypto.is_some() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "encryption already enabled"));
        }
        self.crypto = Some(Crypto {
            enc,
            dec,
            read_scratch: Vec::new(),
            pending: Vec::with_capacity(WRITE_SIZE + 16),
            written: 0,
        });
        Ok(())
    }
}

impl Crypto {
    fn drain<S: AsyncWrite + Unpin>(
        &mut self,
        stream: &mut S,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        while self.written < self.pending.len() {
            let n = ready!(Pin::new(&mut *stream).poll_write(cx, &self.pending[self.written..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.written += n;
        }
        self.pending.clear();
        self.written = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for ClientStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let this = self.get_mut();
        let n = ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        if n > 0
            && let Some(crypto) = &mut this.crypto
        {
            crypto.read_scratch.resize(n + 16, 0);
            let m =
                crypto.dec.update(&buf[..n], &mut crypto.read_scratch).map_err(io::Error::other)?;
            if m != n {
                return Poll::Ready(Err(io::ErrorKind::InvalidData.into()));
            }
            buf[..n].copy_from_slice(&crypto.read_scratch[..n]);
        }
        Poll::Ready(Ok(n))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ClientStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let Some(crypto) = &mut this.crypto else {
            return Pin::new(&mut this.inner).poll_write(cx, buf);
        };
        ready!(crypto.drain(&mut this.inner, cx))?;
        let n = buf.len().min(WRITE_SIZE);
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        crypto.pending.resize(n + 16, 0);
        let m = crypto.enc.update(&buf[..n], &mut crypto.pending).map_err(io::Error::other)?;
        if m != n {
            return Poll::Ready(Err(io::ErrorKind::InvalidData.into()));
        }
        crypto.pending.truncate(m);
        // Acceptance is reported now. Later writes and flush/close drain this
        // buffer before accepting more plaintext; Pending never consumes input.
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(crypto) = &mut this.crypto {
            ready!(crypto.drain(&mut this.inner, cx))?;
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(crypto) = &mut this.crypto {
            ready!(crypto.drain(&mut this.inner, cx))?;
        }
        ready!(Pin::new(&mut this.inner).poll_flush(cx))?;
        Pin::new(&mut this.inner).poll_close(cx)
    }
}

#[cfg(test)]
#[path = "tests/tests_transport.rs"]
mod tests;
