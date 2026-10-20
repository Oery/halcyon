use futures_lite::AsyncWriteExt;

use crate::error::DecodeError;
use crate::packets::PacketWriter;

/// Network NBT value. Only encoding is implemented (registry data is
/// serverbound-only in practice); decoding returns `UnexpectedEof`.
#[derive(Debug, Clone)]
pub enum Nbt<'p> {
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(&'p str),
    List(Vec<Nbt<'p>>),
    Compound(Vec<(&'p str, Nbt<'p>)>),
}

impl Nbt<'_> {
    fn id(&self) -> u8 {
        match self {
            Nbt::Bool(_) | Nbt::Byte(_) => 1,
            Nbt::Short(_) => 2,
            Nbt::Int(_) => 3,
            Nbt::Long(_) => 4,
            Nbt::Float(_) => 5,
            Nbt::Double(_) => 6,
            Nbt::Str(_) => 8,
            Nbt::List(_) => 9,
            Nbt::Compound(_) => 10,
        }
    }

    fn payload_len(&self) -> usize {
        match self {
            Nbt::Bool(_) | Nbt::Byte(_) => 1,
            Nbt::Short(_) => 2,
            Nbt::Int(_) | Nbt::Float(_) => 4,
            Nbt::Long(_) | Nbt::Double(_) => 8,
            Nbt::Str(s) => 2 + s.len(),
            Nbt::List(items) => 1 + 4 + items.iter().map(Nbt::payload_len).sum::<usize>(),
            Nbt::Compound(entries) => {
                entries.iter().map(|(k, v)| 1 + 2 + k.len() + v.payload_len()).sum::<usize>() + 1
            }
        }
    }

    fn write_payload<'a, T: AsyncWriteExt + Unpin + 'a>(
        &'a self,
        w: &'a mut T,
    ) -> std::pin::Pin<Box<dyn Future<Output = crate::encode::Result> + 'a>> {
        Box::pin(async move {
            match self {
                Nbt::Bool(b) => w.write_all(&[*b as u8]).await?,
                Nbt::Byte(b) => w.write_all(&[*b as u8]).await?,
                Nbt::Short(v) => w.write_all(&v.to_be_bytes()).await?,
                Nbt::Int(v) => w.write_all(&v.to_be_bytes()).await?,
                Nbt::Long(v) => w.write_all(&v.to_be_bytes()).await?,
                Nbt::Float(v) => w.write_all(&v.to_be_bytes()).await?,
                Nbt::Double(v) => w.write_all(&v.to_be_bytes()).await?,
                Nbt::Str(s) => {
                    w.write_all(&(s.len() as u16).to_be_bytes()).await?;
                    w.write_all(s.as_bytes()).await?;
                }
                Nbt::List(items) => {
                    let elem = items.first().map(Nbt::id).unwrap_or(0);
                    w.write_all(&[elem]).await?;
                    w.write_all(&(items.len() as i32).to_be_bytes()).await?;
                    for item in items {
                        item.write_payload(w).await?;
                    }
                }
                Nbt::Compound(entries) => {
                    for (k, v) in entries {
                        w.write_all(&[v.id()]).await?;
                        w.write_all(&(k.len() as u16).to_be_bytes()).await?;
                        w.write_all(k.as_bytes()).await?;
                        v.write_payload(w).await?;
                    }
                    w.write_all(&[0]).await?;
                }
            }

            Ok(())
        })
    }
}

impl PacketWriter<'_> for Nbt<'_> {
    fn read(_input: &mut &[u8]) -> Result<Self, DecodeError> {
        Err(DecodeError::UnexpectedEof)
    }

    async fn write<T: AsyncWriteExt + Unpin>(&self, w: &mut T) -> crate::encode::Result {
        w.write_all(&[self.id()]).await?;
        self.write_payload(w).await?;

        Ok(())
    }

    fn body_len(&self) -> usize {
        1 + self.payload_len()
    }
}
