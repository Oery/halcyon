use crate::error::DecodeError;

// TODO: add missing states

#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum State {
    Handshake,
    Status,
    Login,
    Config,
    Play,
}

impl TryFrom<i32> for State {
    type Error = DecodeError;

    fn try_from(val: i32) -> Result<State, Self::Error> {
        match val {
            0 => Ok(State::Handshake),
            1 => Ok(State::Status),
            2 => Ok(State::Login),
            3 => Ok(State::Config),
            4 => Ok(State::Play),
            _ => Err(DecodeError::EnumOutOfRange),
        }
    }
}
