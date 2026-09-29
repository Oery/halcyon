use futures_lite::AsyncWrite;

use crate::VarInt;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::state::State;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;

#[derive(Debug)]
pub struct ServerListPing<'p> {
    pub version: i32,
    pub addr: &'p str,
    pub port: i16,
    pub next_state: State,
}

impl<'p> Payload<'p> for ServerListPing<'p> {
    const ID: VarInt = VarInt(0x00);
    const STATE: State = State::HANDSHAKE;

    fn payload_len(&self) -> usize {
        VarInt(self.version).body_len()
            + self.addr.body_len()
            + self.port.body_len()
            + VarInt(self.next_state as i32).body_len()
    }

    fn decode_payload(buf: &mut &'p [u8]) -> DecodeResult<Self> {
        let version = VarInt::read(buf)?.0;
        let addr: &str = <&str>::read(buf)?;
        let port = i16::read(buf)?;
        let next_state: State = VarInt::read(buf)?.0.try_into()?;

        Ok(ServerListPing { version, addr, port, next_state })
    }

    async fn write_payload<W>(&self, w: &mut W) -> EncodeResult
    where
        W: AsyncWrite + Unpin,
    {
        VarInt(self.version).write(w).await?;
        self.addr.write(w).await?;
        self.port.write(w).await?;
        VarInt(self.next_state as i32).write(w).await?;

        Ok(())
    }
}
