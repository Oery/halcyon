use mc_protocol::decode::take;
use mc_protocol::error::DecodeError;
use mc_protocol::packets::Packets;
use mc_protocol::{PacketWriter, VarInt};

use crate::client::Client;

fn try_read_varint(buf: &mut &[u8]) -> Result<Option<usize>, DecodeError> {
    let mut remaining = *buf;

    match VarInt::read(&mut remaining) {
        Ok(VarInt(value)) => {
            let value = usize::try_from(value).map_err(|_| DecodeError::InvalidVarInt)?;
            *buf = remaining;
            Ok(Some(value))
        }
        Err(DecodeError::UnexpectedEof) => Ok(None),
        Err(err) => Err(err),
    }
}

type DecodeResult<T> = Result<T, DecodeError>;

pub fn read_packet<'p>(
    buf: &'p [u8],
    c: &mut Client,
) -> DecodeResult<Option<(Packets<'p>, usize)>> {
    let mut remaining = buf;

    let Some(frame_len) = try_read_varint(&mut remaining)? else {
        return Ok(None);
    };

    if remaining.len() < frame_len {
        return Ok(None);
    }

    let mut frame = take(&mut remaining, frame_len)?;
    let pkt = Packets::read(&mut frame, &c.ctx)?;

    Ok(Some((pkt, buf.len() - remaining.len())))
}
