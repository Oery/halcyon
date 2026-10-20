use futures_lite::AsyncWrite;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::packets::PacketWriter;
use crate::types::{Nbt, VarInt};

#[derive(Debug)]
pub struct Heightmap {
    pub kind: u8,
    pub data: Vec<i64>,
}

impl<'p> PacketWriter<'p> for Heightmap {
    fn read(buf: &mut &'p [u8]) -> DecodeResult<Self> {
        let kind = u8::read(buf)?;
        let data = <Vec<i64>>::read(buf)?;
        Ok(Self { kind, data })
    }

    async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        self.kind.write(w).await?;
        self.data.write(w).await?;
        Ok(())
    }

    fn body_len(&self) -> usize {
        self.kind.body_len() + self.data.body_len()
    }
}

#[derive(Debug)]
pub struct BlockEntity<'p> {
    pub packed_xz: u8,
    pub y: i16,
    pub kind: i32,
    pub data: Nbt<'p>,
}

impl<'p> PacketWriter<'p> for BlockEntity<'p> {
    fn read(buf: &mut &'p [u8]) -> DecodeResult<Self> {
        let packed_xz = u8::read(buf)?;
        let y = i16::read(buf)?;
        let kind = VarInt::read(buf)?.0;
        let data = Nbt::read(buf)?;
        Ok(Self { packed_xz, y, kind, data })
    }

    async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        self.packed_xz.write(w).await?;
        self.y.write(w).await?;
        VarInt(self.kind).write(w).await?;
        self.data.write(w).await?;
        Ok(())
    }

    fn body_len(&self) -> usize {
        self.packed_xz.body_len()
            + self.y.body_len()
            + VarInt(self.kind).body_len()
            + self.data.body_len()
    }
}

#[derive(Debug)]
pub struct LightData {
    pub sky_mask: Vec<u8>,
    pub block_mask: Vec<u8>,
    pub empty_sky_mask: Vec<u8>,
    pub empty_block_mask: Vec<u8>,
    pub sky_light: Vec<Vec<u8>>,
    pub block_light: Vec<Vec<u8>>,
}

impl PacketWriter<'_> for LightData {
    fn read(buf: &mut &[u8]) -> DecodeResult<Self> {
        let sky_mask = <Vec<u8>>::read(buf)?;
        let block_mask = <Vec<u8>>::read(buf)?;
        let empty_sky_mask = <Vec<u8>>::read(buf)?;
        let empty_block_mask = <Vec<u8>>::read(buf)?;
        let sky_light = <Vec<Vec<u8>>>::read(buf)?;
        let block_light = <Vec<Vec<u8>>>::read(buf)?;
        Ok(Self { sky_mask, block_mask, empty_sky_mask, empty_block_mask, sky_light, block_light })
    }

    async fn write<W: AsyncWrite + Unpin>(&self, w: &mut W) -> EncodeResult {
        self.sky_mask.write(w).await?;
        self.block_mask.write(w).await?;
        self.empty_sky_mask.write(w).await?;
        self.empty_block_mask.write(w).await?;
        self.sky_light.write(w).await?;
        self.block_light.write(w).await?;
        Ok(())
    }

    fn body_len(&self) -> usize {
        self.sky_mask.body_len()
            + self.block_mask.body_len()
            + self.empty_sky_mask.body_len()
            + self.empty_block_mask.body_len()
            + self.sky_light.body_len()
            + self.block_light.body_len()
    }
}
