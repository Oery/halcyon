//! Session admission and delivery of authoritative per-player state.

use std::rc::Rc;
use std::time::Instant;

use mc_protocol::packets::Payload;

use super::Client;
use crate::error::Error;
use crate::lobby::{Profile, Session, Transform};

impl Client {
    pub fn entity_id(&self) -> Result<i32, Error> {
        self.session.as_ref().map(Session::entity_id).ok_or(Error::UnexpectedResponse)
    }

    /// Reserve an authenticated human before its entity ID is sent in Play Login.
    pub async fn reserve_player(&mut self) -> Result<(), Error> {
        use mc_protocol::packets::client::PlayerInfoUpdate;
        use mc_protocol::types::ProfileProperty;

        if self.session.is_some() {
            return Err(Error::UnexpectedResponse);
        }
        let uuid = self.ctx.player_uuid.ok_or(Error::UnexpectedResponse)?;
        let username = self.ctx.username.as_deref().ok_or(Error::UnexpectedResponse)?;
        let properties = self
            .ctx
            .profile_properties
            .iter()
            .map(|p| ProfileProperty {
                name: &p.name,
                value: &p.value,
                signature: p.signature.as_deref(),
            })
            .collect();
        self.packet_buffer.clear();
        PlayerInfoUpdate { uuid, username, properties }
            .write_packet(&mut self.packet_buffer)
            .await?;
        let info: Rc<[u8]> = Rc::from(self.packet_buffer.as_slice());
        let profile = Rc::new(Profile { uuid, username: username.to_owned(), info: info.clone() });
        let (x, y, z, yaw, pitch) = self.server.lobby.spawn;
        self.pos = Transform { x, y, z, yaw, pitch, on_ground: false };
        self.pos = self
            .lobby
            .with_registry(|registry| {
                crate::movement::resolve_spawn(&registry.arena.view(&self.server.lobby), self.pos)
            })
            .ok_or(Error::UnsupportedSpawn)?;
        self.proposed = self.pos;
        self.session = Some(self.lobby.register(profile, self.pos, self.ctx.skin_parts).await?);
        self.profile_info = Some(info);
        // The shared encoded profile is sufficient for subsequent player-info sends.
        self.ctx.profile_properties.clear();
        Ok(())
    }

    pub fn propose_position(&self) {
        if let Some(session) = &self.session {
            session.propose_movement(crate::movement::MovementProposal {
                transform: self.proposed,
                sneaking: self.sneaking,
                sprinting: self.sprinting,
            });
        }
    }

    pub(super) async fn apply_player_update(
        &mut self,
        update: crate::player::PlayerUpdate,
    ) -> Result<(), Error> {
        use crate::lifecycle::{DamageKind, TELEPORT_TIMEOUT};
        use mc_protocol::packets::client::{
            CombatKill, DamageEvent, GameEvent, PlayerAbilities, Respawn, SetContainerContent,
            SetContainerSlot, SetCursorItem, SetHealth, SetHeldItemResponse, SyncPlayerPosition,
        };
        use mc_protocol::types::Nbt;
        if update.respawn {
            self.send(&Respawn {
                dimension_type: 0,
                dimension_name: "minecraft:overworld",
                hashed_seed: 0,
                game_mode: 0,
                previous_game_mode: 0,
                is_debug: false,
                is_flat: false,
                death_location: None,
                portal_cooldown: 0,
                sea_level: 63,
                data_kept: 0,
            })
            .await?;
            // Same-world respawn also restarts the client's level-load gate.
            let center =
                update.teleport.map_or(self.pos.chunk(), |teleport| teleport.destination.chunk());
            self.chunks.resend_center(&self.server.lobby, center);
            self.send(&GameEvent { event: 13, value: 0.0 }).await?;
            self.send(&PlayerAbilities { flags: 0, flying_speed: 0.05, fov_modifier: 0.1 }).await?;
        }
        if let Some(position) = update.block_resync {
            self.resync_blocks(crate::world_sync::resync_cells(position)).await?;
        }
        if let Some(health) = update.health {
            self.send(&SetHealth { health, food: 20, saturation: 5.0 }).await?;
        }
        if let Some(damage) = update.damage {
            let source =
                damage.source.filter(|id| *id > 0).and_then(|id| id.checked_add(1)).unwrap_or(0);
            self.send(&DamageEvent {
                entity_id: self.entity_id()?,
                source_type_id: damage.kind.protocol_id(),
                source_cause_id: source,
                source_direct_id: source,
                source_position: None,
            })
            .await?;
        }
        if let Some(damage) = update.death {
            let message = match damage.kind {
                DamageKind::Fall => "You fell from a high place",
                DamageKind::Void => "You fell out of the world",
                DamageKind::Suffocation => "You suffocated in a wall",
                DamageKind::Game => "You died",
            };
            self.send(&CombatKill { player_id: self.entity_id()?, message: Nbt::Str(message) })
                .await?;
        }
        if let Some(inventory) = update.inventory {
            if update.inventory_full {
                self.send(&SetContainerContent {
                    window_id: 0,
                    state_id: inventory.state_id,
                    slots: &inventory.slots,
                    carried: &inventory.cursor,
                })
                .await?;
            } else {
                for (slot, stack) in inventory.slots.iter().enumerate() {
                    if update.inventory_slots & (1u64 << slot) != 0 {
                        self.send(&SetContainerSlot {
                            window_id: 0,
                            state_id: inventory.state_id,
                            slot: slot as i16,
                            stack,
                        })
                        .await?;
                    }
                }
            }
            self.send(&SetCursorItem { stack: &inventory.cursor }).await?;
            if update.inventory_full || self.selected_slot != inventory.selected {
                self.send(&SetHeldItemResponse { slot: i32::from(inventory.selected) }).await?;
                self.selected_slot = inventory.selected;
            }
        }
        if let Some(teleport) = update.teleport {
            let p = teleport.destination;
            self.pos = p;
            self.proposed = p;
            self.sneaking = false;
            self.sprinting = false;
            self.chunks.reconcile(&self.server.lobby, p.chunk());
            self.send(&SyncPlayerPosition {
                teleport_id: teleport.id,
                x: p.x,
                y: p.y,
                z: p.z,
                vel_x: 0.0,
                vel_y: 0.0,
                vel_z: 0.0,
                yaw: p.yaw,
                pitch: p.pitch,
                flags: 0,
            })
            .await?;
            self.pending_teleport = Some(teleport.id);
            self.teleport_deadline = Some(Instant::now() + TELEPORT_TIMEOUT);
        }
        Ok(())
    }
}
