use futures_lite::AsyncWrite;

use crate::VarInt;
use crate::data::config::*;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::state::State;

use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;

use macros::{Payload, packet};
use uuid::Uuid;

#[packet(Handshake, 0x00, Server)]
pub struct ServerListPing<'p> {
    #[format = "varint"]
    pub version: i32,
    pub addr: &'p str,
    pub port: u16,
    #[format = "varint"]
    pub next_state: State,
}

#[packet(Status, 0x00, Server)]
pub struct StatusRequest;

#[packet(Status, 0x01, Server)]
pub struct PingRequest {
    pub time: i64,
}

#[packet(Login, 0x00, Server)]
pub struct LoginStart<'p> {
    pub name: &'p str,
    pub uuid: Uuid,
}

#[packet(Login, 0x01, Server)]
pub struct EncryptionResponse<'p> {
    pub shared_secret: &'p [u8],
    pub verify_token: &'p [u8],
}

#[packet(Login, 0x02, Server)]
pub struct LoginPluginResponse<'p> {
    #[format = "varint"]
    pub message_id: i32,
    pub data: &'p [u8],
}

#[packet(Login, 0x03, Server)]
pub struct LoginAcknowledge;

#[packet(Login, 0x04, Server)]
pub struct LoginCookieResponse<'p> {
    pub ident: &'p str,
    pub data: &'p [u8],
}

#[packet(Config, 0x00, Server)]
pub struct ClientInformation<'p> {
    pub locale: &'p str,
    pub render_distance: u8,
    #[format = "varint"]
    pub chat_mode: ChatMode,
    pub chat_colors: bool,
    pub skin_parts: SkinParts,
    #[format = "varint"]
    pub main_hand: Hand,
    pub text_filtering: bool,
    pub allow_server_listing: bool,
    #[format = "varint"]
    pub particle_status: ParticleStatus,
}

#[packet(Config, 0x01, Server)]
pub struct ConfigCookieResponse<'p> {
    pub key: &'p str,
    pub payload: Option<&'p [u8]>,
}

#[packet(Config, 0x03, Server)]
pub struct FinishConfig;

#[packet(Config, 0x04, Server)]
pub struct ConfigKeepAlive {
    pub id: i64,
}

#[packet(Config, 0x06, Server)]
pub struct ConfigResourcePackResponse {
    pub uuid: Uuid,
    #[format = "varint"]
    pub result: ResourcePackResult,
}

#[packet(Config, 0x09, Server)]
pub struct AcceptCodeOfConduct;

#[packet(Play, 0x03, Server)]
pub struct BundleItemSelected {
    #[format = "varint"]
    pub slot_of_bundle: i32,
    #[format = "varint"]
    pub slot_in_bundle: i32,
}

#[packet(Play, 0x06, Server)]
pub struct ChatAcknowledgement {
    #[format = "varint"]
    pub message_count: i32,
}

#[packet(Play, 0x0D, Server)]
pub struct ClientTickEnd;

#[packet(Play, 0x0F, Server)]
pub struct CommandSuggestion<'p> {
    #[format = "varint"]
    pub transaction_id: i32,
    pub text: &'p str,
}

#[packet(Play, 0x10, Server)]
pub struct AcknowledgeConfiguration;

#[packet(Play, 0x2C, Server)]
pub struct PlayerLoaded;

#[packet(Play, 0x2E, Server)]
pub struct Punch;
