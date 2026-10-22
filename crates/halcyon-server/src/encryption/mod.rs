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
#[path = "../tests/tests_encryption.rs"]
mod tests;
