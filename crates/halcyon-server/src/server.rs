use crate::encryption::Keys;

pub struct Server {
    pub keys: Keys,
}

impl Server {
    pub fn new() -> Result<Server, openssl::error::ErrorStack> {
        Ok(Server { keys: Keys::new()? })
    }
}
