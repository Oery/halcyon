use futures_lite::AsyncWrite;

use crate::State;
use crate::data::motd::Status;
use crate::decode::Result as DecodeResult;
use crate::encode::Result as EncodeResult;
use crate::packets::PacketWriter;
use crate::packets::Payload;
use crate::types::{
    BlockEntity, GameProfile, Heightmap, KnownPack, LightData, RegistryEntry, TaggedRegistry,
    VarInt, bytes::Remainder,
};

use macros::{Payload, packet};
use uuid::Uuid;

// TODO: how to handle borrowing here
#[packet(Status, 0x00, Client)]
pub struct StatusPacket<'p> {
    #[format = "json"]
    pub json: Status<'p>,
}

#[packet(Status, 0x01, Client)]
pub struct PingResponse {
    pub time: i64,
}

#[packet(Login, 0x00, Client)]
pub struct LoginDisconnect<'p> {
    pub reason: &'p str,
}

#[packet(Login, 0x01, Client)]
pub struct EncryptionRequest<'p> {
    pub server_id: &'p str,
    pub public_key: &'p [u8],
    pub verify_token: &'p [u8],
    pub should_authenticate: bool,
}

#[packet(Login, 0x02, Client)]
pub struct LoginSuccess<'p> {
    pub profile: GameProfile<'p>,
    pub session_id: Uuid,
}

#[packet(Login, 0x03, Client)]
pub struct SetCompression {
    #[format = "varint"]
    pub threshold: i32,
}

#[packet(Login, 0x04, Client)]
pub struct LoginPluginRequest<'p> {
    #[format = "varint"]
    pub message_id: i32,
    pub channel: &'p str,
    pub data: &'p [u8],
}

#[packet(Login, 0x05, Client)]
pub struct LoginCookieRequest<'p> {
    pub key: &'p str,
}

#[packet(Config, 0x00, Client)]
pub struct ConfigCookieRequest<'p> {
    pub key: &'p str,
}

#[packet(Config, 0x01, Client)]
pub struct PluginMessage<'p> {
    pub ident: &'p str,
    pub data: Remainder<'p>,
}

// TODO: packets with same content should be merged
// TODO: reason should be a text component
#[packet(Config, 0x02, Client)]
pub struct ConfigDisconnect<'p> {
    pub reason: &'p str,
}

#[packet(Config, 0x04, Client)]
pub struct ConfigKeepAlive {
    pub id: i64,
}

#[packet(Config, 0x06, Client)]
pub struct ResetChat;

#[packet(Config, 0x14, Client)]
pub struct CodeOfConduct<'p> {
    pub coc: &'p str,
}

// NOTE: unused by vanilla server
// #[packet(Config, 0x05, Client)]
// pub struct ConfigPing {
//     pub id: i32,
// }

#[packet(Config, 0x08, Client)]
pub struct RemoveResourcePack {
    pub uuid: Option<Uuid>,
}

#[packet(Config, 0x0B, Client)]
pub struct StoreCookie<'p> {
    pub key: &'p str,
    pub payload: &'p [u8],
}

// TODO: enforce max sizes in parsing

#[packet(Config, 0x0C, Client)]
pub struct Transfer<'p> {
    pub host: &'p str,
    #[format = "varint"]
    pub port: i32,
}

#[packet(Config, 0x12, Client)]
pub struct ClearDialog;

#[packet(Play, 0x00, Client)]
pub struct BundleDelimiter;

#[packet(Play, 0x04, Client)]
pub struct AcknowledgeBlockChange {
    #[format = "varint"]
    pub sequence_id: i32,
}

#[packet(Play, 0x0B, Client)]
pub struct ChunkBatchFinished {
    #[format = "varint"]
    pub batch_size: i32,
}

#[packet(Play, 0x0C, Client)]
pub struct ChunkBatchStart;

#[packet(Play, 0x2D, Client)]
pub struct PlayKeepAlive {
    pub id: i64,
}

#[packet(Play, 0x27, Client)]
pub struct GameEvent {
    pub event: u8,
    pub value: f32,
}

#[packet(Play, 0x41, Client)]
pub struct PlayerAbilities {
    pub flags: u8,
    pub flying_speed: f32,
    pub fov_modifier: f32,
}

#[packet(Play, 0x63, Client)]
pub struct SetDefaultSpawnPosition<'p> {
    pub dimension_name: &'p str,
    pub location: i64,
    pub yaw: f32,
    pub pitch: f32,
}

#[packet(Play, 0x6A, Client)]
pub struct SetHealth {
    pub health: f32,
    #[format = "varint"]
    pub food: i32,
    pub saturation: f32,
}

#[packet(Play, 0x60, Client)]
pub struct SetCenterChunk {
    #[format = "varint"]
    pub chunk_x: i32,
    #[format = "varint"]
    pub chunk_z: i32,
}
#[packet(Play, 0x49, Client)]
pub struct SyncPlayerPosition {
    #[format = "varint"]
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub vel_x: f64,
    pub vel_y: f64,
    pub vel_z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: i32,
}

#[packet(Play, 0x2E, Client)]
pub struct ChunkData<'p> {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub heightmaps: Vec<Heightmap>,
    pub data: Vec<u8>,
    pub block_entities: Vec<BlockEntity<'p>>,
    pub light: LightData,
}

#[packet(Play, 0x0E, Client)]
pub struct ClearTitles {
    pub reset: bool,
}

#[packet(Play, 0x33, Client)]
pub struct LowDiskSpaceWarning;

#[packet(Play, 0x43, Client)]
pub struct EndCombat {
    #[format = "varint"]
    pub duration: i32,
}

#[packet(Play, 0x44, Client)]
pub struct EnterCombat;

#[packet(Play, 0x78, Client)]
pub struct StartConfiguration;

#[packet(Config, 0x0F, Client)]
pub struct KnownPacks<'p> {
    pub packs: Vec<KnownPack<'p>>,
}

#[packet(Config, 0x07, Client)]
pub struct RegistryData<'p> {
    pub registry: &'p str,
    pub entries: Vec<RegistryEntry<'p>>,
}

#[packet(Config, 0x0D, Client)]
pub struct FeatureFlags<'p> {
    pub flags: Vec<&'p str>,
}

#[packet(Config, 0x0E, Client)]
pub struct UpdateTags<'p> {
    pub registries: Vec<TaggedRegistry<'p>>,
}

#[packet(Config, 0x03, Client)]
pub struct FinishConfiguration;

pub struct PlayLogin<'p> {
    pub entity_id: i32,
    pub hardcore: bool,
    pub dimension_names: Vec<&'p str>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub do_limited_crafting: bool,
    pub dimension_type: VarInt,
    pub dimension_name: &'p str,
    pub hashed_seed: i64,
    pub game_mode: VarInt,
    pub previous_game_mode: VarInt,
    pub is_debug: bool,
    pub is_flat: bool,
    pub has_death_location: bool,
    pub portal_cooldown: VarInt,
    pub sea_level: VarInt,
    pub enforces_secure_chat: bool,
}

impl Default for PlayLogin<'_> {
    fn default() -> Self {
        Self {
            entity_id: 1,
            hardcore: false,
            dimension_names: vec!["minecraft:overworld"],
            max_players: 20,
            view_distance: 8,
            simulation_distance: 8,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            do_limited_crafting: false,
            dimension_type: VarInt(0),
            dimension_name: "minecraft:overworld",
            hashed_seed: 0,
            game_mode: VarInt(0),
            previous_game_mode: VarInt(0),
            is_debug: false,
            is_flat: false,
            has_death_location: false,
            portal_cooldown: VarInt(0),
            sea_level: VarInt(63),
            enforces_secure_chat: false,
        }
    }
}

impl<'p> Payload<'p> for PlayLogin<'p> {
    const ID: VarInt = VarInt(0x32);
    const STATE: State = State::Play;

    fn payload_len(&self) -> usize {
        let mut length = 0;
        length += self.entity_id.body_len();
        length += self.hardcore.body_len();
        length += self.dimension_names.body_len();
        length += VarInt(self.max_players).body_len();
        length += VarInt(self.view_distance).body_len();
        length += VarInt(self.simulation_distance).body_len();
        length += self.reduced_debug_info.body_len();
        length += self.enable_respawn_screen.body_len();
        length += self.do_limited_crafting.body_len();
        length += self.dimension_type.body_len();
        length += self.dimension_name.body_len();
        length += self.hashed_seed.body_len();
        length += self.game_mode.body_len();
        length += self.previous_game_mode.body_len();
        length += self.is_debug.body_len();
        length += self.is_flat.body_len();
        length += self.has_death_location.body_len();
        length += self.portal_cooldown.body_len();
        length += self.sea_level.body_len();
        length += false.body_len();
        length += self.enforces_secure_chat.body_len();
        length
    }

    fn decode_payload(buf: &mut &'p [u8]) -> DecodeResult<Self> {
        let entity_id = i32::read(buf)?;
        let hardcore = bool::read(buf)?;
        let dimension_names = <Vec<&str>>::read(buf)?;
        let max_players = VarInt::read(buf)?.0;
        let view_distance = VarInt::read(buf)?.0;
        let simulation_distance = VarInt::read(buf)?.0;
        let reduced_debug_info = bool::read(buf)?;
        let enable_respawn_screen = bool::read(buf)?;
        let do_limited_crafting = bool::read(buf)?;
        let dimension_type = VarInt::read(buf)?;
        let dimension_name = <&str>::read(buf)?;
        let hashed_seed = i64::read(buf)?;
        let game_mode = VarInt::read(buf)?;
        let previous_game_mode = VarInt::read(buf)?;
        let is_debug = bool::read(buf)?;
        let is_flat = bool::read(buf)?;
        let has_death_location = bool::read(buf)?;
        if has_death_location {
            return Err(crate::error::DecodeError::EnumOutOfRange);
        }
        let portal_cooldown = VarInt::read(buf)?;
        let sea_level = VarInt::read(buf)?;
        let _online_mode = bool::read(buf)?;
        let enforces_secure_chat = bool::read(buf)?;

        Ok(Self {
            entity_id,
            hardcore,
            dimension_names,
            max_players,
            view_distance,
            simulation_distance,
            reduced_debug_info,
            enable_respawn_screen,
            do_limited_crafting,
            dimension_type,
            dimension_name,
            hashed_seed,
            game_mode,
            previous_game_mode,
            is_debug,
            is_flat,
            has_death_location,
            portal_cooldown,
            sea_level,
            enforces_secure_chat,
        })
    }

    async fn write_payload<W>(&self, w: &mut W) -> EncodeResult
    where
        W: AsyncWrite + Unpin,
    {
        self.entity_id.write(w).await?;
        self.hardcore.write(w).await?;
        self.dimension_names.write(w).await?;
        VarInt(self.max_players).write(w).await?;
        VarInt(self.view_distance).write(w).await?;
        VarInt(self.simulation_distance).write(w).await?;
        self.reduced_debug_info.write(w).await?;
        self.enable_respawn_screen.write(w).await?;
        self.do_limited_crafting.write(w).await?;
        self.dimension_type.write(w).await?;
        self.dimension_name.write(w).await?;
        self.hashed_seed.write(w).await?;
        self.game_mode.write(w).await?;
        self.previous_game_mode.write(w).await?;
        self.is_debug.write(w).await?;
        self.is_flat.write(w).await?;
        self.has_death_location.write(w).await?;
        self.portal_cooldown.write(w).await?;
        self.sea_level.write(w).await?;
        true.write(w).await?;
        self.enforces_secure_chat.write(w).await?;

        Ok(())
    }
}
