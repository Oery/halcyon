use futures_lite::AsyncWrite;
use serde::{Deserialize, Serialize};

use crate::State;
use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::types::VarInt;

use macros::{Payload, packet};

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
