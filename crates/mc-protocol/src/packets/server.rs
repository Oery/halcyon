use futures_lite::AsyncWrite;

use crate::VarInt;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::state::State;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;

use macros::{Payload, packet};
use uuid::Uuid;

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

#[packet(Login, 0x00, Server)]
pub struct LoginStart<'p> {
    pub name: &'p str,
    pub uuid: Uuid,
}

#[packet(Login, 0x01, Server)]
pub struct EncryptionResponse<'p> {
    pub shared_secret: &'p [u8],
    pub verify_token: &'p [u8],
}

#[packet(Login, 0x02, Server)]
pub struct LoginPluginResponse<'p> {
    #[format = "varint"]
    pub message_id: i32,
    pub data: &'p [u8],
}

#[packet(Login, 0x03, Server)]
pub struct LoginAcknowledge;
