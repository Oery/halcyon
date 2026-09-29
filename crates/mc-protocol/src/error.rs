use std::io;
use std::str;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EncodeError {
    #[error("io error")]
    IOError(#[from] io::Error),

    #[error("can't encode json")]
    InvalidJSON(#[from] serde_json::Error),
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("expected bytes but got eof")]
    UnexpectedEof,

    #[error("varint exceeds 32 bits")]
    InvalidVarInt,

    #[error("string is not valid utf8")]
    InvalidString(#[from] str::Utf8Error),

    #[error("packet id is unknown")]
    UnknownPacket,

    #[error("enum variant is out of range")]
    EnumOutOfRange,

    #[error("json is not valid")]
    InvalidJSON(#[from] serde_json::Error),
}
