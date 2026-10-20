use openssl::error::ErrorStack;
use openssl::pkey::Private;
use openssl::rand::rand_bytes;
use openssl::rsa::{Padding, Rsa};
use openssl::sha::Sha1;
use openssl::symm::{Cipher, Crypter, Mode};

pub struct Keys {
    pub public: Vec<u8>,
    pub private: Rsa<Private>,
}

impl Keys {
    pub fn new() -> Result<Keys, ErrorStack> {
        let private = Rsa::generate(2048)?;
        let public = private.public_key_to_der()?;

        Ok(Keys { public, private })
    }
}

pub fn generate_verify_token() -> Result<[u8; 4], ErrorStack> {
    let mut token = [0u8; 4];
    rand_bytes(&mut token)?;
    Ok(token)
}

pub fn decrypt_rsa(data: &[u8], private: &Rsa<Private>) -> Result<Vec<u8>, ErrorStack> {
    let mut decrypted = vec![0u8; private.size() as usize];
    let len = private.private_decrypt(data, &mut decrypted, Padding::PKCS1)?;

    decrypted.truncate(len);
    Ok(decrypted)
}

/// Notch-style server hash: `SHA1(server_id + shared_secret + public_key)`
/// rendered as a signed two's-complement hex string.
pub fn server_hash(server_id: &str, shared_secret: &[u8], public_key: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(shared_secret);
    hasher.update(public_key);
    notch_hex(&hasher.finish())
}

fn notch_hex(digest: &[u8; 20]) -> String {
    let mut negative = digest[0] & 0x80 != 0;
    let mut bytes = *digest;

    if negative {
        let mut carry = true;
        for b in bytes.iter_mut().rev() {
            let (v, c) = (!*b).overflowing_add(carry as u8);
            *b = v;
            carry = c;
        }
        negative = true;
    }

    let mut hex = String::with_capacity(41);
    if negative {
        hex.push('-');
    }

    let mut started = false;
    for b in bytes {
        for nibble in [(b >> 4) & 0xf, b & 0xf] {
            if nibble != 0 {
                started = true;
            }
            if started {
                // Nibble is 0..=15 by masking; `from_digit` is infallible here.
                hex.push(char::from_digit(nibble as u32, 16).unwrap_or('0'));
            }
        }
    }

    if !started {
        hex.push('0');
    }

    hex
}

/// AES/CFB8 stream cipher pair keyed by the shared secret (key == IV).
pub fn ciphers(shared_secret: &[u8; 16]) -> Result<(Crypter, Crypter), ErrorStack> {
    let mut enc =
        Crypter::new(Cipher::aes_128_cfb8(), Mode::Encrypt, shared_secret, Some(shared_secret))?;
    enc.pad(false);
    let mut dec =
        Crypter::new(Cipher::aes_128_cfb8(), Mode::Decrypt, shared_secret, Some(shared_secret))?;
    dec.pad(false);

    Ok((enc, dec))
}

#[cfg(test)]
mod tests {
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
}
