use futures_lite::AsyncWrite;

use crate::error::DecodeError;
use crate::packets::client::Ping;
use crate::packets::server::ServerListPing;
use crate::state::State;
use crate::types::VarInt;

pub mod client;
pub mod server;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;

use macros::{Payload, packet};

// TODO: add compression
pub struct Context {
    pub state: State,
}

#[derive(Debug)]
pub enum Packets<'p> {
    ServerListPing(ServerListPing<'p>),
    PingRequest(Ping),
    StatusRequest(StatusRequest),
}

impl<'p> Packets<'p> {
    pub fn read(buf: &mut &'p [u8], ctx: &Context) -> Result<Packets<'p>, DecodeError> {
        let id = VarInt::read(buf)?.0;

        match (id, ctx.state) {
            (0x00, State::HANDSHAKE) => {
                Ok(Packets::ServerListPing(ServerListPing::decode_payload(buf)?))
            }
            (0x00, State::STATUS) => Ok(Packets::StatusRequest(StatusRequest)),
            (0x01, State::STATUS) => Ok(Packets::PingRequest(Ping::decode_payload(buf)?)),
            _ => Err(DecodeError::UnknownPacket),
        }
    }
}

pub trait PacketWriter<'p>: Sized {
    fn read(buf: &mut &'p [u8]) -> DecodeResult<Self>;

    async fn write<T: AsyncWrite + Unpin>(self, w: &mut T) -> EncodeResult;

    fn body_len(&self) -> usize;
}

pub trait Payload<'p>: Sized {
    const ID: VarInt;
    const STATE: State;

    fn payload_len(&self) -> usize;

    fn decode_payload(buf: &mut &'p [u8]) -> DecodeResult<Self>;

    async fn write_payload<W>(&self, w: &mut W) -> EncodeResult
    where
        W: AsyncWrite + Unpin;

    async fn write_packet<'a, W>(&self, w: &mut W) -> EncodeResult
    where
        W: AsyncWrite + Unpin,
    {
        let length = Self::ID.body_len() + self.payload_len();
        VarInt(length as i32).write(w).await?;

        Self::ID.write(w).await?;

        self.write_payload(w).await?;

        Ok(())
    }
}

#[packet(Status, 0x00, Client)]
pub struct StatusRequest;
