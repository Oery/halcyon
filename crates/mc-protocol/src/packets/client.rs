use futures_lite::AsyncWrite;
use serde::{Deserialize, Serialize};

use crate::State;
use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::types::{GameProfile, VarInt};

use macros::{Payload, packet};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug)]
pub struct Version<'a> {
    #[serde(borrow)]
    pub name: &'a str,
    pub protocol: i32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Description<'a> {
    #[serde(borrow)]
    pub text: &'a str,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Player<'a> {
    #[serde(borrow)]
    pub name: &'a str,
    #[serde(borrow)]
    pub id: &'a str,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Players<'a> {
    pub max: i32,
    pub online: i32,
    #[serde(borrow)]
    pub sample: Vec<Player<'a>>,
}

// FIXME: description should be a text component
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status<'c> {
    #[serde(borrow)]
    pub version: Version<'c>,
    pub players: Players<'c>,
    pub description: Description<'c>,
    #[serde(borrow)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon: Option<&'c str>,
    pub enforces_secure_chat: bool,
}

// TODO: how to handle borrowing here
#[packet(Status, 0x00, Client)]
pub struct StatusPacket<'p> {
    #[format = "json"]
    pub json: Status<'p>,
}

#[packet(Status, 0x01, Client)]
pub struct PingResponse {
    pub time: i64,
}

#[packet(Login, 0x00, Client)]
pub struct LoginDisconnect<'p> {
    pub reason: &'p str,
}

#[packet(Login, 0x01, Client)]
pub struct EncryptionRequest<'p> {
    pub server_id: &'p str,
    pub public_key: &'p [u8],
    pub verify_token: &'p [u8],
    pub should_authenticate: bool,
}

#[packet(Login, 0x02, Client)]
pub struct LoginSuccess<'p> {
    pub profile: GameProfile<'p>,
    pub session_id: Uuid,
}

#[packet(Login, 0x03, Client)]
pub struct SetCompression {
    #[format = "varint"]
    pub threshold: i32,
}

#[packet(Login, 0x04, Client)]
pub struct LoginPluginRequest<'p> {
    #[format = "varint"]
    pub message_id: i32,
    pub channel: &'p str,
    pub data: &'p [u8],
}

#[packet(Login, 0x05, Client)]
pub struct LoginCookieRequest<'p> {
    pub key: &'p str,
}
