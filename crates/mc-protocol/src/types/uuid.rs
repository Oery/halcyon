use futures_lite::AsyncWriteExt;
use uuid::Uuid;

use crate::decode::take;
use crate::error::DecodeError;
use crate::packets::PacketWriter;

impl<'p> PacketWriter<'p> for Uuid {
    fn read(input: &mut &'p [u8]) -> Result<Self, DecodeError> {
        let bytes: [u8; 16] = take(input, 16)?.try_into().expect("buffer should contains 16 bytes");
        Ok(Uuid::from_bytes(bytes))
    }

    async fn write<W: AsyncWriteExt + Unpin>(&self, w: &mut W) -> crate::encode::Result {
        w.write_all(self.as_bytes()).await?;
        Ok(())
    }

    fn body_len(&self) -> usize {
        16
    }
}
