use super::*;
use mc_protocol::State;

#[test]
fn framing_distinguishes_fragmentation_from_invalid_payloads() {
    let ctx = Context { state: State::Status, ..Default::default() };
    assert!(read_packet(&[0x80], &ctx).unwrap().is_none());
    assert!(read_packet(&[9, 1, 0], &ctx).unwrap().is_none());
    assert!(matches!(read_packet(&[1, 1], &ctx), Err(DecodeError::UnexpectedEof)));
    assert!(matches!(read_packet(&[2, 0, 1], &ctx), Err(DecodeError::TrailingData)));
    for bytes in [&[0][..], &[0x80, 0x80, 0x80], &[0xff, 0xff, 0xff]] {
        assert!(read_packet(bytes, &ctx).is_err());
    }
    let bytes = [1, 0x7f, 1, 0];
    let (unknown, used) = read_packet(&bytes, &ctx).unwrap().unwrap();
    assert!(unknown.is_none());
    let (known, used_again) = read_packet(&bytes[used..], &ctx).unwrap().unwrap();
    assert!(matches!(known, Some(Packets::StatusRequest(_))));
    assert_eq!(used + used_again, bytes.len());
}

#[test]
fn frames_larger_than_the_initial_receive_buffer_decode() {
    let ctx = Context { state: State::Config, ..Default::default() };
    let mut frame = vec![0x85, 0x08, 0x02, 1, b'x'];
    frame.resize(1031, 0); // 1029-byte plugin message body.
    assert_eq!(read_packet(&frame, &ctx).unwrap().unwrap().1, frame.len());
}
