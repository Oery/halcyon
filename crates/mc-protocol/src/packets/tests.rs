use super::*;

fn encode<T: Payload<'static>>(v: &T) -> Vec<u8> {
    futures_lite::future::block_on(async {
        let mut buf = Vec::new();
        v.write_packet(&mut buf).await.unwrap();
        buf
    })
}

#[test]
fn encryption_response_dispatches_in_login_state() {
    let ctx = Context { state: State::Login, ..Default::default() };
    // id 0x01 + empty secret + empty token
    let raw = [0x01u8, 0x00, 0x00];
    let mut slice = raw.as_slice();

    assert!(matches!(Packets::read(&mut slice, &ctx).unwrap(), Packets::EncryptionResponse(_)));
    assert!(slice.is_empty());
}

#[test]
fn movement_flags_dispatches_in_play_and_preserves_collision_bit() {
    let ctx = Context { state: State::Play, ..Default::default() };
    let raw = [0x21, 0x03];
    let mut input = raw.as_slice();
    let Packets::SetPlayerMovementFlags(packet) = Packets::read(&mut input, &ctx).unwrap() else {
        panic!("incorrect movement packet");
    };
    assert!(input.is_empty());
    assert_eq!(packet.flags, 0x03);
    assert_eq!(encode(&packet), [0x02, 0x21, 0x03]);
    assert!(SetPlayerMovementFlags::decode_payload(&mut [].as_slice()).is_err());
}

#[test]
fn login_success_round_trips_profile_and_session() {
    use crate::types::{GameProfile, ProfileProperty};

    let profile = GameProfile {
        uuid: Uuid::nil(),
        username: "Notch",
        properties: vec![ProfileProperty { name: "textures", value: "abc", signature: None }],
    };
    let packet = LoginSuccess { profile, session_id: Uuid::nil() };
    let bytes = encode(&packet);

    // frame: len + id(0x02) + 16 uuid + str + props + 16 session
    let mut frame = bytes.as_slice();
    let len = VarInt::read(&mut frame).unwrap().0 as usize;
    assert_eq!(len, bytes.len() - 1);
    assert_eq!(frame[0], 0x02);
}
