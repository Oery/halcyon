use crate::encryption::Keys;

pub struct Server {
    pub keys: Keys,
    pub lobby: halcyon_world::World,
}

impl Server {
    pub fn new(lobby: halcyon_world::World) -> Result<Server, openssl::error::ErrorStack> {
        Ok(Server { keys: Keys::new()?, lobby })
    }
}
