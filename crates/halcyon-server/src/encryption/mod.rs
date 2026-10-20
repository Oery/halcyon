use openssl::error::ErrorStack;
use openssl::pkey::Private;
use openssl::rand::rand_bytes;
use openssl::rsa::{Padding, Rsa};

pub struct Keys {
    pub public: Vec<u8>,
    pub private: Rsa<Private>,
}

impl Keys {
    pub fn new() -> Keys {
        let private = Rsa::generate(2048).unwrap();
        let public = private.public_key_to_der().unwrap();

        Keys { public, private }
    }
}

pub fn generate_verify_token() -> Result<[u8; 4], ErrorStack> {
    let mut token = [0u8; 4];
    rand_bytes(&mut token)?;
    Ok(token)
}

pub fn decrypt_rsa(data: &[u8], private: &Rsa<Private>) -> [u8; 2048] {
    let mut decrypted = [0; 2048];

    private.private_decrypt(data, &mut decrypted, Padding::PKCS1).unwrap();

    decrypted
}
