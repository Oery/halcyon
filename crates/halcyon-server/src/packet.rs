use mc_protocol::decode::{MAX_FRAME_LEN, take};
use mc_protocol::error::DecodeError;
use mc_protocol::packets::{Context, Packets};

/// Only the prefix/body boundary can be incomplete. Once the declared frame
/// is available, all decoding errors are fatal, including UnexpectedEof.
pub fn read_packet<'p>(
    buf: &'p [u8],
    ctx: &Context,
) -> Result<Option<(Option<Packets<'p>>, usize)>, DecodeError> {
    let mut frame_len = 0;
    let mut prefix_len = 0;
    for shift in [0, 7, 14] {
        let Some(&byte) = buf.get(prefix_len) else {
            return Ok(None);
        };
        prefix_len += 1;
        frame_len |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        if prefix_len == 3 {
            return Err(DecodeError::InvalidVarInt);
        }
    }
    if frame_len == 0 || frame_len > MAX_FRAME_LEN {
        return Err(DecodeError::InvalidLength);
    }
    let mut remaining = &buf[prefix_len..];
    if remaining.len() < frame_len {
        return Ok(None);
    }
    let mut frame = take(&mut remaining, frame_len)?;
    let packet = match Packets::read(&mut frame, ctx) {
        Ok(packet) => {
            if !frame.is_empty() {
                return Err(DecodeError::TrailingData);
            }
            Some(packet)
        }
        Err(DecodeError::UnknownPacket(_)) => None,
        Err(error) => return Err(error),
    };
    Ok(Some((packet, prefix_len + frame_len)))
}

#[cfg(test)]
#[path = "tests/tests_packet.rs"]
mod tests;
