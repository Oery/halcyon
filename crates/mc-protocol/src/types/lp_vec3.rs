//! Protocol 777's quantized vector, checked against the official LpVec3 codec.
use super::VarInt;
use crate::{
    decode::Result as DecodeResult, encode::Result as EncodeResult, packets::PacketWriter,
};
use futures_lite::{AsyncWrite, AsyncWriteExt};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LpVec3(pub [f64; 3]);

impl LpVec3 {
    fn packed(self) -> Option<(u64, u64)> {
        let values = self.0.map(|v| {
            if v.is_nan() {
                0.0
            } else {
                v.clamp(-17_179_869_183.0, 17_179_869_183.0)
            }
        });
        let max = values.into_iter().map(f64::abs).fold(0.0, f64::max);
        if max < 1.0 / 32766.0 {
            return None;
        }
        let scale = max.ceil() as u64;
        let mut packed = (scale & 3) | if scale > 3 { 4 } else { 0 };
        for (value, offset) in values.into_iter().zip([3, 18, 33]) {
            let component = ((value / scale as f64 * 0.5 + 0.5) * 32766.0 + 0.5).floor() as u64;
            packed |= component << offset;
        }
        Some((packed, scale))
    }
}

impl PacketWriter<'_> for LpVec3 {
    fn read(input: &mut &[u8]) -> DecodeResult<Self> {
        let first = u8::read(input)?;
        if first == 0 {
            return Ok(Self::default());
        }
        let packed = u64::from(first)
            | (u64::from(u8::read(input)?) << 8)
            | ((i32::read(input)? as u32 as u64) << 16);
        let mut scale = u64::from(first & 3);
        if first & 4 != 0 {
            scale |= (VarInt::read(input)?.0 as u32 as u64) << 2;
        }
        Ok(Self([3, 18, 33].map(|offset| {
            (((packed >> offset) & 32767).min(32766) as f64 * 2.0 / 32766.0 - 1.0) * scale as f64
        })))
    }
    async fn write<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> EncodeResult {
        let Some((packed, scale)) = self.packed() else {
            return 0u8.write(writer).await;
        };
        writer.write_all(&[packed as u8, (packed >> 8) as u8]).await?;
        ((packed >> 16) as i32).write(writer).await?;
        if scale > 3 {
            VarInt((scale >> 2) as i32).write(writer).await?;
        }
        Ok(())
    }
    fn body_len(&self) -> usize {
        self.packed().map_or(1, |(_, scale)| {
            6 + if scale > 3 {
                VarInt((scale >> 2) as i32).body_len()
            } else {
                0
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vectors_match_official_26_3_codec_and_bound_quantization() {
        // Bytes captured from net.minecraft.network.LpVec3.write, including
        // continuation scale and Java's NaN/infinity sanitization.
        for (values, expected) in [
            ([0.0, 0.0, 0.0], vec![0]),
            ([0.0, -0.0392, 0.0], vec![0xf9, 0xff, 0x7f, 0xfe, 0xf5, 0xf5]),
            ([0.0, -1.95, 0.0], vec![0xfa, 0xff, 0x7f, 0xfe, 0x06, 0x69]),
            ([0.25, -4.5, 1.2], vec![0x95, 0x19, 0x9e, 0xb6, 0x19, 0x9a, 1]),
            (
                [f64::NAN, f64::INFINITY, f64::NEG_INFINITY],
                vec![0xff, 0xff, 0, 1, 0xff, 0xf9, 0xff, 0xff, 0xff, 0xff, 0x0f],
            ),
        ] {
            let vector = LpVec3(values);
            let mut bytes = Vec::new();
            futures_lite::future::block_on(vector.write(&mut bytes)).unwrap();
            assert_eq!(bytes, expected);
            assert_eq!(bytes.len(), vector.body_len());
            let mut input = bytes.as_slice();
            let decoded = LpVec3::read(&mut input).unwrap();
            assert!(input.is_empty());
            let scale = values
                .into_iter()
                .filter(|v| v.is_finite())
                .map(f64::abs)
                .fold(1.0, f64::max)
                .ceil();
            if values.iter().all(|v| v.is_finite()) {
                for (actual, wanted) in decoded.0.into_iter().zip(values) {
                    assert!((actual - wanted).abs() <= scale / 32766.0 + 1e-12);
                }
            }
            for length in 0..bytes.len() {
                assert!(LpVec3::read(&mut &bytes[..length]).is_err());
            }
        }
    }
}
