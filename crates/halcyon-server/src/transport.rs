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
mod tests {
    use super::*;
    use crate::encryption::ciphers;
    use std::collections::VecDeque;
    use std::task::Waker;

    #[derive(Default)]
    struct Writer {
        limits: VecDeque<Option<usize>>,
        bytes: Vec<u8>,
        closed: bool,
    }
    impl AsyncWrite for Writer {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let Some(limit) = self.limits.pop_front().unwrap_or(Some(buf.len())) else {
                return Poll::Pending;
            };
            let n = limit.min(buf.len());
            self.bytes.extend_from_slice(&buf[..n]);
            Poll::Ready(Ok(n))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_close(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.closed = true;
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn encrypted_writes_survive_pending_short_writes_and_close() {
        let secret = [7; 16];
        let (enc, dec) = ciphers(&secret).unwrap();
        let mut stream = ClientStream::new(Writer {
            limits: [None, Some(2), None].into(),
            ..Default::default()
        });
        stream.enable_encryption(enc, dec).unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(Pin::new(&mut stream).poll_write(&mut cx, b"hello"), Poll::Ready(Ok(5))));
        assert!(Pin::new(&mut stream).poll_write(&mut cx, b"world").is_pending());
        assert!(Pin::new(&mut stream).poll_flush(&mut cx).is_pending());
        assert_eq!(stream.inner.bytes.len(), 2);
        assert!(matches!(Pin::new(&mut stream).poll_write(&mut cx, b"world"), Poll::Ready(Ok(5))));
        assert!(matches!(Pin::new(&mut stream).poll_close(&mut cx), Poll::Ready(Ok(()))));
        assert!(stream.inner.closed);
        let (_, mut dec) = ciphers(&secret).unwrap();
        let mut plain = [0; 32];
        let n = dec.update(&stream.inner.bytes, &mut plain).unwrap();
        assert_eq!(&plain[..n], b"helloworld");
    }

    #[test]
    fn zero_write_is_reported_instead_of_spinning() {
        let (enc, dec) = ciphers(&[0; 16]).unwrap();
        let mut stream =
            ClientStream::new(Writer { limits: [Some(0)].into(), ..Default::default() });
        stream.enable_encryption(enc, dec).unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(Pin::new(&mut stream).poll_write(&mut cx, b"x"), Poll::Ready(Ok(1))));
        assert!(
            matches!(Pin::new(&mut stream).poll_flush(&mut cx), Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::WriteZero)
        );
    }

    #[test]
    fn encrypted_reads_keep_cipher_state_and_reuse_scratch() {
        struct Reader(VecDeque<Option<Vec<u8>>>);
        impl AsyncRead for Reader {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &mut [u8],
            ) -> Poll<io::Result<usize>> {
                match self.0.pop_front() {
                    Some(Some(bytes)) => {
                        buf[..bytes.len()].copy_from_slice(&bytes);
                        Poll::Ready(Ok(bytes.len()))
                    }
                    Some(None) => Poll::Pending,
                    None => Poll::Ready(Ok(0)),
                }
            }
        }
        let (mut enc, _) = ciphers(&[3; 16]).unwrap();
        let mut parts = VecDeque::new();
        for plain in [b"hello", b"world"] {
            let mut bytes = vec![0; plain.len() + 16];
            let n = enc.update(plain, &mut bytes).unwrap();
            bytes.truncate(n);
            parts.push_back(None);
            parts.push_back(Some(bytes));
        }
        let (enc, dec) = ciphers(&[3; 16]).unwrap();
        let mut stream = ClientStream::new(Reader(parts));
        stream.enable_encryption(enc, dec).unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        let mut buf = [0; 8];
        assert!(Pin::new(&mut stream).poll_read(&mut cx, &mut buf).is_pending());
        assert_eq!(stream.crypto.as_ref().unwrap().read_scratch.capacity(), 0);
        assert!(matches!(Pin::new(&mut stream).poll_read(&mut cx, &mut buf), Poll::Ready(Ok(5))));
        assert_eq!(&buf[..5], b"hello");
        let capacity = stream.crypto.as_ref().unwrap().read_scratch.capacity();
        assert!(Pin::new(&mut stream).poll_read(&mut cx, &mut buf).is_pending());
        assert!(matches!(Pin::new(&mut stream).poll_read(&mut cx, &mut buf), Poll::Ready(Ok(5))));
        assert_eq!(&buf[..5], b"world");
        assert_eq!(stream.crypto.as_ref().unwrap().read_scratch.capacity(), capacity);
    }
}
