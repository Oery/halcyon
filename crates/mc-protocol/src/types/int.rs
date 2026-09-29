use futures_lite::AsyncWriteExt;

use crate::decode::take;
use crate::error::DecodeError;
use crate::packets::PacketWriter;

// TODO: is a short encoded as little endian or big endian?

impl PacketWriter<'_> for i16 {
    fn read(input: &mut &[u8]) -> Result<Self, DecodeError> {
        let bytes: [u8; 2] = take(input, 2)?.try_into().expect("buffer should contains 2 bytes");

        return Ok(i16::from_be_bytes(bytes));
    }

    async fn write<W: AsyncWriteExt + Unpin>(self, w: &mut W) -> crate::encode::Result {
        let bytes: [u8; 2] = self.to_be_bytes();
        w.write_all(&bytes).await?;

        Ok(())
    }

    fn body_len(&self) -> usize {
        2
    }
}

impl PacketWriter<'_> for i64 {
    fn read(input: &mut &[u8]) -> Result<Self, DecodeError> {
        let bytes: [u8; 8] = take(input, 8)?.try_into().expect("buffer should contains 4 bytes");

        return Ok(i64::from_be_bytes(bytes));
    }

    async fn write<W: AsyncWriteExt + Unpin>(self, w: &mut W) -> crate::encode::Result {
        w.write_all(&self.to_be_bytes()).await?;
        Ok(())
    }

    fn body_len(&self) -> usize {
        8
    }
}
