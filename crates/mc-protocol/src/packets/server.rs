use futures_lite::AsyncWrite;

use crate::VarInt;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::state::State;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;

use macros::{Payload, packet};

#[packet(Handshake, 0x00, Server)]
pub struct ServerListPing<'p> {
    #[format = "varint"]
    pub version: i32,
    pub addr: &'p str,
    pub port: i16,
    #[format = "varint"]
    pub next_state: State,
}

#[packet(Status, 0x00, Server)]
pub struct StatusRequest;

#[packet(Status, 0x01, Server)]
pub struct PingRequest {
    pub time: i64,
}
