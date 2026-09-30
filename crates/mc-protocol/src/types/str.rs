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

    async fn write<T: AsyncWriteExt + Unpin>(&self, w: &mut T) -> crate::encode::Result {
        VarInt(self.len() as i32).write(w).await?;
        w.write_all(self.as_bytes()).await?;

        Ok(())
    }

    fn body_len(&self) -> usize {
        self.len() + VarInt(self.len() as i32).body_len()
    }
}

impl<'p, T: PacketWriter<'p>> PacketWriter<'p> for Vec<T> {
    fn read(buf: &mut &'p [u8]) -> Result<Self, DecodeError> {
        let length = VarInt::read(buf)?.0;
        let mut elements = Vec::with_capacity(length as usize);

        for _ in 0..length {
            let element = T::read(buf)?;
            elements.push(element);
        }

        Ok(elements)
    }

    async fn write<W: AsyncWriteExt + Unpin>(&self, w: &mut W) -> crate::encode::Result {
        VarInt(self.body_len() as i32).write(w).await?;

        for e in self {
            e.write(w).await?;
        }

        Ok(())
    }

    fn body_len(&self) -> usize {
        let mut length = 0;

        length += VarInt(self.len() as i32).body_len();
        length += self.iter().fold(0, |len, e| len + e.body_len());

        length
    }
}
