use futures_lite::AsyncWriteExt;

use crate::decode::take;
use crate::error::DecodeError;
use crate::packets::PacketWriter;
use crate::types::VarInt;

impl<'p> PacketWriter<'p> for &'p [u8] {
    fn read(input: &mut &'p [u8]) -> Result<&'p [u8], DecodeError> {
        let len = VarInt::read(input)?.0 as usize;
        let bytes = take(input, len)?;
        Ok(bytes)
    }

    async fn write<T: AsyncWriteExt + Unpin>(&self, w: &mut T) -> crate::encode::Result {
        VarInt(self.len() as i32).write(w).await?;
        w.write_all(self).await?;

        Ok(())
    }

    fn body_len(&self) -> usize {
        self.len() + VarInt(self.len() as i32).body_len()
    }
}
