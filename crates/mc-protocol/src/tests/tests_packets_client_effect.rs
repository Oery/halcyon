use super::*;

fn encode<'p>(packet: &impl Payload<'p>) -> Vec<u8> {
    let mut bytes = Vec::new();
    futures_lite::future::block_on(packet.write_payload(&mut bytes)).unwrap();
    assert_eq!(bytes.len(), packet.payload_len().unwrap());
    bytes
}

#[test]
fn swing_animation_carries_hand_kind_and_duration() {
    let packet = SwingAnimation {
        entity_id: 300,
        hand: InteractionHand::Off as i32,
        kind: SwingKind::Whack as i32,
        duration: SWING_TICKS,
    };
    assert_eq!(encode(&packet), [0xac, 0x02, 1, 1, 6]);
}

#[test]
fn inline_sound_event_round_trips_without_a_registry_id() {
    let packet = SoundEffect {
        sound: InlineSound { name: "a:b", fixed_range: None },
        category: sound_category::PLAYER,
        x: 8,
        y: -16,
        z: 0,
        volume: 1.0,
        pitch: 1.0,
        seed: 7,
    };
    let bytes = encode(&packet);
    assert_eq!(&bytes[..7], [0, 3, b'a', b':', b'b', 0, 7]);
    assert_eq!(&bytes[7..11], 8i32.to_be_bytes());
    assert_eq!(&bytes[11..15], (-16i32).to_be_bytes());
    assert_eq!(&bytes[bytes.len() - 8..], 7i64.to_be_bytes());
    let mut input = bytes.as_slice();
    let sound = InlineSound::read(&mut input).unwrap();
    assert_eq!((sound.name, sound.fixed_range), ("a:b", None));
}

#[test]
fn level_particles_put_the_particle_type_first() {
    let packet = LevelParticles {
        particle: particle::SWEEP_ATTACK,
        override_limiter: false,
        always_visible: false,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        offset_x: 0.0,
        offset_y: 0.0,
        offset_z: 0.0,
        speed_x: 0.0,
        speed_y: 0.0,
        speed_z: 0.0,
        count: 2,
        randomization: 0,
    };
    let bytes = encode(&packet);
    assert_eq!(bytes.len(), 1 + 2 + 24 + 24 + 1 + 1);
    assert_eq!(bytes[0], 77);
    assert_eq!(&bytes[bytes.len() - 2..], [2, 0]);
}
