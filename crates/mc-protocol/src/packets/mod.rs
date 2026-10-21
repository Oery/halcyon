use futures_lite::AsyncWrite;
use uuid::Uuid;

use crate::auth::Property;
use crate::error::DecodeError;
use crate::packets::client::*;
use crate::packets::server::*;
use crate::state::State;
use crate::types::VarInt;

pub mod client;
pub mod server;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;

use macros::InnerPacket;

#[derive(Default, Debug)]
pub struct Context {
    pub state: State,
    pub compression: Option<i32>,
    pub verify_token: Option<[u8; 4]>,
    pub username: Option<String>,
    pub player_uuid: Option<Uuid>,
    /// Mojang `textures` property (+ signature) from `hasJoined`, kept for
    /// the Play-time `PlayerInfoUpdate` that actually renders the skin.
    pub profile_properties: Vec<Property>,
    /// `Displayed Skin Parts` bitmask from `Client Information` (sent in
    /// Config and again on every Play settings change). Echoed back as
    /// player metadata index 16 so layers/cape render.
    pub skin_parts: u8,
}

pub trait InnerPacket {
    fn id(&self) -> i32;
    fn state(&self) -> State;
}

#[derive(InnerPacket, Debug)]
pub enum Packets<'p> {
    ServerListPing(ServerListPing<'p>),
    PingRequest(PingRequest),
    PingResponse(PingResponse),
    StatusRequest(StatusRequest),
    SetCompression(SetCompression),
    LoginStart(LoginStart<'p>),
    EncryptionResponse(EncryptionResponse<'p>),
    LoginPluginResponse(LoginPluginResponse<'p>),
    LoginAcknowledge(LoginAcknowledge),
    LoginCookieResponse(LoginCookieResponse<'p>),
    ClientInformation(ClientInformation<'p>),
    ConfigPluginMessage(ConfigPluginMessage<'p>),
    AcknowledgeFinish(AcknowledgeFinish),
    ConfigKnownPacks(ConfigKnownPacks<'p>),
    ConfirmTeleportation(ConfirmTeleportation),
    PlayKeepAliveResponse(PlayKeepAliveResponse),
    ChunkBatchReceived(ChunkBatchReceived),
    SetPlayerPosition(SetPlayerPosition),
    SetPlayerPositionRotation(SetPlayerPositionRotation),
    SetPlayerRotation(SetPlayerRotation),
    SetPlayerMovementFlags(SetPlayerMovementFlags),
    ClientTickEnd(ClientTickEnd),
    PlayerLoaded(PlayerLoaded),
    UseItemOn(UseItemOn),
    UseItem(UseItem),
    PlayClientInformation(ClientInformation<'p>),
    ClientCommand(ClientCommand),
    ClickContainer(ClickContainer),
    CloseContainer(CloseContainer),
    SetHeldItem(SetHeldItem),
    PlayerAction(PlayerAction),
    PlayerCommand(PlayerCommand),
    PlayerInput(PlayerInput),
}

impl<'p> Packets<'p> {
    pub fn read(buf: &mut &'p [u8], ctx: &Context) -> Result<Packets<'p>, DecodeError> {
        let id = VarInt::read(buf)?.0;

        match (id, ctx.state) {
            (0x0C, State::Play) => Ok(Packets::ClientCommand(ClientCommand::decode_payload(buf)?)),
            (0x12, State::Play) => {
                Ok(Packets::ClickContainer(ClickContainer::decode_payload(buf)?))
            }
            (0x13, State::Play) => {
                Ok(Packets::CloseContainer(CloseContainer::decode_payload(buf)?))
            }
            (0x36, State::Play) => Ok(Packets::SetHeldItem(SetHeldItem::decode_payload(buf)?)),
            (0x29, State::Play) => Ok(Packets::PlayerAction(PlayerAction::decode_payload(buf)?)),
            (0x2A, State::Play) => Ok(Packets::PlayerCommand(PlayerCommand::decode_payload(buf)?)),
            (0x2B, State::Play) => Ok(Packets::PlayerInput(PlayerInput::decode_payload(buf)?)),
            (0x42, State::Play) => Ok(Packets::UseItemOn(UseItemOn::decode_payload(buf)?)),
            (0x43, State::Play) => Ok(Packets::UseItem(UseItem::decode_payload(buf)?)),
            (0x00, State::Handshake) => {
                Ok(Packets::ServerListPing(ServerListPing::decode_payload(buf)?))
            }
            (0x00, State::Status) => Ok(Packets::StatusRequest(StatusRequest)),
            (0x01, State::Status) => Ok(Packets::PingRequest(PingRequest::decode_payload(buf)?)),
            (0x00, State::Login) => Ok(Packets::LoginStart(LoginStart::decode_payload(buf)?)),
            (0x01, State::Login) => {
                Ok(Packets::EncryptionResponse(EncryptionResponse::decode_payload(buf)?))
            }
            (0x02, State::Login) => {
                Ok(Packets::LoginPluginResponse(LoginPluginResponse::decode_payload(buf)?))
            }
            (0x03, State::Login) => Ok(Packets::LoginAcknowledge(LoginAcknowledge)),
            (0x04, State::Login) => {
                Ok(Packets::LoginCookieResponse(LoginCookieResponse::decode_payload(buf)?))
            }
            (0x00, State::Config) => {
                Ok(Packets::ClientInformation(ClientInformation::decode_payload(buf)?))
            }
            (0x02, State::Config) => {
                Ok(Packets::ConfigPluginMessage(ConfigPluginMessage::decode_payload(buf)?))
            }
            (0x03, State::Config) => Ok(Packets::AcknowledgeFinish(AcknowledgeFinish)),
            (0x07, State::Config) => {
                Ok(Packets::ConfigKnownPacks(ConfigKnownPacks::decode_payload(buf)?))
            }
            (0x00, State::Play) => {
                Ok(Packets::ConfirmTeleportation(ConfirmTeleportation::decode_payload(buf)?))
            }
            (0x1C, State::Play) => {
                Ok(Packets::PlayKeepAliveResponse(PlayKeepAliveResponse::decode_payload(buf)?))
            }
            (0x0B, State::Play) => {
                Ok(Packets::ChunkBatchReceived(ChunkBatchReceived::decode_payload(buf)?))
            }
            (0x1E, State::Play) => {
                Ok(Packets::SetPlayerPosition(SetPlayerPosition::decode_payload(buf)?))
            }
            (0x1F, State::Play) => Ok(Packets::SetPlayerPositionRotation(
                SetPlayerPositionRotation::decode_payload(buf)?,
            )),
            (0x20, State::Play) => {
                Ok(Packets::SetPlayerRotation(SetPlayerRotation::decode_payload(buf)?))
            }
            (0x21, State::Play) => {
                Ok(Packets::SetPlayerMovementFlags(SetPlayerMovementFlags::decode_payload(buf)?))
            }
            (0x0D, State::Play) => Ok(Packets::ClientTickEnd(ClientTickEnd)),
            (0x0E, State::Play) => {
                Ok(Packets::PlayClientInformation(ClientInformation::decode_payload(buf)?))
            }
            (0x2C, State::Play) => Ok(Packets::PlayerLoaded(PlayerLoaded)),
            _ => Err(DecodeError::UnknownPacket(id)),
        }
    }
}

// These traits deliberately support local, non-Send transports (Glommio).
#[allow(async_fn_in_trait)]
pub trait PacketWriter<'p>: Sized {
    fn read(buf: &mut &'p [u8]) -> DecodeResult<Self>;

    async fn write<T: AsyncWrite + Unpin>(&self, w: &mut T) -> EncodeResult;

    fn body_len(&self) -> usize;
}

#[allow(async_fn_in_trait)]
pub trait Payload<'p>: Sized {
    const ID: VarInt;
    const STATE: State;

    fn payload_len(&self) -> EncodeResult<usize>;

    fn decode_payload(buf: &mut &'p [u8]) -> DecodeResult<Self>;

    async fn write_payload<W>(&self, w: &mut W) -> EncodeResult
    where
        W: AsyncWrite + Unpin;

    async fn write_packet<W>(&self, w: &mut W) -> EncodeResult
    where
        W: AsyncWrite + Unpin,
    {
        let length = Self::ID.body_len() + self.payload_len()?;
        VarInt(length as i32).write(w).await?;

        Self::ID.write(w).await?;

        self.write_payload(w).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode<T: Payload<'static>>(v: &T) -> Vec<u8> {
        futures_lite::future::block_on(async {
            let mut buf = Vec::new();
            v.write_packet(&mut buf).await.unwrap();
            buf
        })
    }

    #[test]
    fn encryption_response_dispatches_in_login_state() {
        let ctx = Context { state: State::Login, ..Default::default() };
        // id 0x01 + empty secret + empty token
        let raw = [0x01u8, 0x00, 0x00];
        let mut slice = raw.as_slice();

        assert!(matches!(Packets::read(&mut slice, &ctx).unwrap(), Packets::EncryptionResponse(_)));
        assert!(slice.is_empty());
    }

    #[test]
    fn movement_flags_dispatches_in_play_and_preserves_collision_bit() {
        let ctx = Context { state: State::Play, ..Default::default() };
        let raw = [0x21, 0x03];
        let mut input = raw.as_slice();
        let Packets::SetPlayerMovementFlags(packet) = Packets::read(&mut input, &ctx).unwrap()
        else {
            panic!("incorrect movement packet");
        };
        assert!(input.is_empty());
        assert_eq!(packet.flags, 0x03);
        assert_eq!(encode(&packet), [0x02, 0x21, 0x03]);
        assert!(SetPlayerMovementFlags::decode_payload(&mut [].as_slice()).is_err());
    }

    #[test]
    fn login_success_round_trips_profile_and_session() {
        use crate::types::{GameProfile, ProfileProperty};

        let profile = GameProfile {
            uuid: Uuid::nil(),
            username: "Notch",
            properties: vec![ProfileProperty { name: "textures", value: "abc", signature: None }],
        };
        let packet = LoginSuccess { profile, session_id: Uuid::nil() };
        let bytes = encode(&packet);

        // frame: len + id(0x02) + 16 uuid + str + props + 16 session
        let mut frame = bytes.as_slice();
        let len = VarInt::read(&mut frame).unwrap().0 as usize;
        assert_eq!(len, bytes.len() - 1);
        assert_eq!(frame[0], 0x02);
    }
}
