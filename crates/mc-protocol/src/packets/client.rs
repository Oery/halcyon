use serde::{Deserialize, Serialize};

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::types::VarInt;

use macros::Payload;
use macros::packet;

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

#[packet(Status, 0x00, Client)]
pub struct StatusPacket<'p> {
    #[format = "json"]
    pub json: Status<'p>,
}

use futures_lite::AsyncWrite;

use crate::State;

impl<'p> Payload<'p> for Status<'p> {
    const ID: VarInt = VarInt(0x00);
    const STATE: State = State::HANDSHAKE;

    fn payload_len(&self) -> usize {
        let json = serde_json::to_string(&self).unwrap();
        <&str>::body_len(&json.as_str())
    }

    fn decode_payload(buf: &mut &'p [u8]) -> crate::decode::Result<Self> {
        let json = <&str>::read(buf)?;
        let status: Status = serde_json::from_str(json)?;

        Ok(status)
    }

    async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> crate::encode::Result {
        let json = serde_json::to_string(&self)?;
        json.write(w).await?;

        Ok(())
    }
}

#[packet(Play, 0x00, Client)]
pub struct Ping {
    pub time: i64,
}

// impl<'p> Payload<'p> for Ping {
//     const ID: VarInt = VarInt(0x01);
//     const STATE: State = State::STATUS;
//
//     fn payload_len(&self) -> usize {
//         self.time.body_len()
//     }
//
//     fn decode_payload(buf: &mut &'p [u8]) -> crate::decode::Result<Self> {
//         let time = i64::read(buf)?;
//         Ok(Ping { time })
//     }
//
//     async fn write_payload<W: AsyncWrite + Unpin>(&self, w: &mut W) -> crate::encode::Result {
//         self.time.write(w).await?;
//
//         Ok(())
//     }
// }
