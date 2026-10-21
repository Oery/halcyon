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
    let mut stream =
        ClientStream::new(Writer { limits: [None, Some(2), None].into(), ..Default::default() });
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
    let mut stream = ClientStream::new(Writer { limits: [Some(0)].into(), ..Default::default() });
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
