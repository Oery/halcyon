use crate::encryption::Keys;

pub struct Server {
    pub keys: Keys,
}

impl Server {
    pub fn new() -> Server {
        let keys = Keys::new();

        Server { keys }
    }
}
