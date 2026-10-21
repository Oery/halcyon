use super::*;

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finish()
}

#[test]
fn notch_vectors_match_wiki() {
    // https://minecraft.wiki/w/Protocol_encryption § Server Hashes
    assert_eq!(notch_hex(&sha1(b"Notch")), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
    assert_eq!(notch_hex(&sha1(b"jeb_")), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
}

#[test]
fn rsa_round_trip() {
    let keys = Keys::new().expect("test key generation works");
    let secret = *b"0123456789abcdef";

    let mut encrypted = vec![0u8; keys.private.size() as usize];
    let n = keys.private.public_encrypt(&secret, &mut encrypted, Padding::PKCS1).unwrap();
    let decrypted = decrypt_rsa(&encrypted[..n], &keys.private).unwrap();

    assert_eq!(decrypted, secret);
}

#[test]
fn cfb8_streams_continuously() {
    let secret = *b"0123456789abcdef";
    let (mut enc, mut dec) = ciphers(&secret).unwrap();

    let mut ct = vec![0u8; 64];
    let n = enc.update(b"hello", &mut ct).unwrap();
    let mut ct2 = vec![0u8; 64];
    let m = enc.update(b"world", &mut ct2).unwrap();

    let mut pt = vec![0u8; 64];
    let a = dec.update(&ct[..n], &mut pt).unwrap();
    let mut pt2 = vec![0u8; 64];
    let b = dec.update(&ct2[..m], &mut pt2).unwrap();

    assert_eq!(&pt[..a], b"hello");
    assert_eq!(&pt2[..b], b"world");
}
