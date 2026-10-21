use mc_protocol::error::EncodeError;
use thiserror::Error;

/// Failures for one client connection. Write/crypto failures drop the
/// connection; only [`Error::disconnect_reason`] justifies a `LoginDisconnect`.
#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("packet encode failed: {0}")]
    Encode(#[from] EncodeError),

    #[error("world load failed: {0}")]
    World(#[from] halcyon_world::LoadError),

    #[error("crypto operation failed: {0}")]
    Crypto(#[from] openssl::error::ErrorStack),

    #[error("unexpected encryption response")]
    UnexpectedResponse,

    #[error("encryption response without login start")]
    MissingStart,

    #[error("could not decrypt shared secret")]
    DecryptSecret,

    #[error("could not decrypt verify token")]
    DecryptToken,

    #[error("verify token mismatch")]
    TokenMismatch,

    #[error("shared secret has wrong length")]
    BadSecretLen,

    #[error("mojang auth rejected the login")]
    VerifyFailed,

    #[error("mojang profile name mismatch")]
    NameMismatch,

    #[error("mojang profile id is not a valid uuid")]
    BadProfileId,
}

impl Error {
    /// Player-facing reason, if the failure justifies a `LoginDisconnect`.
    pub fn disconnect_reason(&self) -> Option<&'static str> {
        match self {
            Error::VerifyFailed | Error::NameMismatch => Some("Failed to verify username!"),
            _ => None,
        }
    }
}
