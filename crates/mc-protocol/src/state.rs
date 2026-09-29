use crate::error::DecodeError;

// TODO: add missing states

#[repr(i32)]
#[derive(Clone, Copy, Debug)]
pub enum State {
    HANDSHAKE,
    STATUS,
    LOGIN,
    TRANSFER,
}

impl TryFrom<i32> for State {
    type Error = DecodeError;

    fn try_from(val: i32) -> Result<State, Self::Error> {
        match val {
            0 => Ok(State::HANDSHAKE),
            1 => Ok(State::STATUS),
            2 => Ok(State::LOGIN),
            3 => Ok(State::TRANSFER),
            _ => Err(DecodeError::EnumOutOfRange),
        }
    }
}
