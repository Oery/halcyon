use futures_lite::AsyncWriteExt;

use crate::decode::take;
use crate::error::DecodeError;
use crate::packets::PacketWriter;
use crate::types::VarInt;

impl<'p> PacketWriter<'p> for &'p str {
    fn read(input: &mut &'p [u8]) -> Result<&'p str, DecodeError> {
        let len = VarInt::read(input)?.0 as usize;
        let bytes = take(input, len)?;
        let str = str::from_utf8(&bytes)?;
        Ok(str)
    }

    async fn write<T: AsyncWriteExt + Unpin>(self, w: &mut T) -> crate::encode::Result {
        VarInt(self.len() as i32).write(w).await?;
        w.write_all(self.as_bytes()).await?;

        Ok(())
    }

    fn body_len(&self) -> usize {
        self.len() + VarInt(self.len() as i32).body_len()
    }
}
