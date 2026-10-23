//! Session admission and delivery of authoritative per-player state.

use std::rc::Rc;
use std::time::Instant;

use mc_protocol::packets::Payload;

use super::Client;
use crate::error::Error;
use crate::instance::{Profile, Session};
use crate::inventory::InventorySlot;

impl Client {
    pub async fn reject_queued_block_predictions(&mut self) -> Result<(), Error> {
        let Some(session) = &self.session else {
            return Ok(());
        };
        // Drain the unsent authority and cancel queued actions without suspending.
        let cells = session.cancel_block_actions();
        let mut update = session.take_update();
        update.inventory = self
            .instance
            .with_registry(|registry| registry.inventory_snapshot(session.entity_id()));
        update.inventory_full = true;
        self.apply_player_update(update).await?;
        self.resync_blocks(cells).await
    }
    pub async fn send_abilities(&mut self) -> Result<(), Error> {
        self.send(&mc_protocol::packets::client::PlayerAbilities {
            flags: self.game_mode.ability_flags(),
            flying_speed: 0.05,
            fov_modifier: 0.1,
        })
        .await
    }
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
        self.pos = self.instance.map().spawn();
        self.pos = self
            .instance
            .with_registry(|registry| {
                crate::movement::resolve_spawn(
                    &registry.arena.view(self.instance.world()),
                    self.pos,
                )
            })
            .ok_or(Error::UnsupportedSpawn)?;
        self.proposed = self.pos;
        self.session = Some(self.instance.register(profile, self.pos, self.ctx.skin_parts).await?);
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
            CombatKill, GameEvent, Respawn, SetContainerContent, SetContainerSlot, SetCursorItem,
            SetEntityVelocity, SetHealth, SetHeldItemResponse, SyncPlayerPosition,
        };
        use mc_protocol::types::{LpVec3, Nbt};
        use std::borrow::Cow;
        let mode_changed =
            update.game_mode.is_some_and(|mode| mode.current != self.game_mode.current);
        if let Some(mode) = update.game_mode {
            self.game_mode = mode;
        }
        if update.respawn {
            self.send(&Respawn {
                dimension_type: 0,
                dimension_name: "minecraft:overworld",
                hashed_seed: 0,
                game_mode: self.game_mode.current as i32,
                previous_game_mode: self.game_mode.previous.map_or(0, |mode| mode as i32 + 1),
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
            self.chunks.resend_center(self.instance.world(), center);
            self.send(&GameEvent { event: 13, value: 0.0 }).await?;
        }
        if mode_changed {
            self.send(&GameEvent { event: 3, value: self.game_mode.current as i32 as f32 }).await?;
        }
        if update.game_mode.is_some() || update.respawn {
            self.send_abilities().await?;
        }
        if let Some(position) = update.block_resync {
            self.resync_blocks(crate::world_sync::resync_cells(position)).await?;
        }
        // Authoritative states must precede the acknowledgement, which ends prediction.
        if !update.used_blocks.is_empty() {
            self.resync_blocks(update.used_blocks).await?;
        }
        if let Some(health) = update.health {
            self.send(&SetHealth { health, food: 20, saturation: 5.0 }).await?;
        }
        if let Some(damage) = update.damage {
            self.send(&damage.event(self.entity_id()?)).await?;
        }
        if let Some(damage) = update.death {
            let message: Cow<str> = match damage.kind {
                DamageKind::Fall => "You fell from a high place".into(),
                DamageKind::Void => "You fell out of the world".into(),
                DamageKind::Suffocation => "You suffocated in a wall".into(),
                DamageKind::PlayerAttack => match &update.killer {
                    Some(killer) => format!("You were slain by {killer}").into(),
                    None => "You were slain".into(),
                },
                DamageKind::Game => "You died".into(),
            };
            self.send(&CombatKill { player_id: self.entity_id()?, message: Nbt::Str(&message) })
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
                for (slot, stack) in InventorySlot::all().zip(&inventory.slots) {
                    if update.inventory_slots & slot.mask() != 0 {
                        self.send(&SetContainerSlot {
                            window_id: 0,
                            state_id: inventory.state_id,
                            slot: slot.protocol_id(),
                            stack,
                        })
                        .await?;
                    }
                }
            }
            self.send(&SetCursorItem { stack: &inventory.cursor }).await?;
            if update.inventory_full || self.selected_slot != inventory.selected {
                self.send(&SetHeldItemResponse {
                    slot: i32::from(inventory.selected.protocol_id()),
                })
                .await?;
                self.selected_slot = inventory.selected;
            }
        }
        if let Some(sequence_id) = update.block_ack {
            self.send(&mc_protocol::packets::client::AcknowledgeBlockChange { sequence_id })
                .await?;
        }
        // The camera precedes its teleport, as in vanilla.
        if let Some(camera) = update.camera {
            self.camera = camera;
            let camera_id = camera.map_or_else(|| self.entity_id(), Ok)?;
            self.send(&mc_protocol::packets::client::SetCamera { camera_id }).await?;
        }
        if let Some(teleport) = update.teleport {
            let p = teleport.destination;
            self.pos = p;
            self.proposed = p;
            self.sneaking = false;
            self.sprinting = false;
            self.chunks.reconcile(self.instance.world(), p.chunk());
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
        } else if let Some(velocity) = update.velocity {
            // A teleport already stops the client's motion.
            self.send(&SetEntityVelocity {
                entity_id: self.entity_id()?,
                velocity: LpVec3(velocity),
            })
            .await?;
        }
        Ok(())
    }
}
