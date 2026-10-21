//! Encrypted memory receiver for bounded gameplay publication workloads.

use std::collections::HashSet;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_lite::{AsyncWrite, AsyncWriteExt, future::block_on};
use mc_protocol::packets::{Payload, client::*};

use crate::encryption::ciphers;
use crate::lobby::Registry;
use crate::player::PlayerUpdate;
use crate::transport::ClientStream;
use crate::visibility::{Observer, WRITE_SLICE_BYTES, WRITE_SLICE_FRAMES};

#[derive(Default)]
pub(super) struct Sink {
    pub(super) bytes: u64,
}
impl AsyncWrite for Sink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes += bytes.len() as u64;
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub(super) struct Receiver {
    pub(super) observer: Observer,
    pub(super) world_observer: crate::world_sync::WorldObserver,
    pub(super) terrain: HashSet<(i32, i32)>,
    pub(super) stream: ClientStream<Sink>,
    scratch: Vec<u8>,
}

impl Receiver {
    pub(super) fn new(terrain: HashSet<(i32, i32)>) -> Self {
        let (enc, dec) = ciphers(&[7; 16]).unwrap();
        let mut stream = ClientStream::new(Sink::default());
        stream.enable_encryption(enc, dec).unwrap();
        Self {
            observer: Observer::default(),
            world_observer: Default::default(),
            terrain,
            stream,
            scratch: Vec::with_capacity(1024),
        }
    }

    fn packet<'p>(&mut self, packet: &impl Payload<'p>) {
        block_on(async {
            self.scratch.clear();
            packet.write_packet(&mut self.scratch).await.unwrap();
            self.stream.write_all(&self.scratch).await.unwrap();
            self.stream.flush().await.unwrap();
        });
    }

    pub(super) fn self_update(&mut self, update: PlayerUpdate) {
        if update.respawn {
            self.packet(&Respawn {
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
            });
            self.packet(&GameEvent { event: 13, value: 0.0 });
            self.packet(&PlayerAbilities { flags: 0, flying_speed: 0.05, fov_modifier: 0.1 });
        }
        if let Some(health) = update.health {
            self.packet(&SetHealth { health, food: 20, saturation: 5.0 });
        }
        if update.damage.is_some() {
            self.packet(&DamageEvent {
                entity_id: 1,
                source_type_id: 18,
                source_cause_id: 0,
                source_direct_id: 0,
                source_position: None,
            });
        }
        if update.death.is_some() {
            self.packet(&CombatKill {
                player_id: 1,
                message: mc_protocol::types::Nbt::Str("You died"),
            });
        }
        if let Some(inventory) = update.inventory {
            if update.inventory_full {
                self.packet(&SetContainerContent {
                    window_id: 0,
                    state_id: inventory.state_id,
                    slots: &inventory.slots,
                    carried: &inventory.cursor,
                });
            } else {
                for (slot, stack) in inventory.slots.iter().enumerate() {
                    if update.inventory_slots & (1 << slot) != 0 {
                        self.packet(&SetContainerSlot {
                            window_id: 0,
                            state_id: inventory.state_id,
                            slot: slot as i16,
                            stack,
                        });
                    }
                }
            }
            self.packet(&SetCursorItem { stack: &inventory.cursor });
            self.packet(&SetHeldItemResponse { slot: i32::from(inventory.selected) });
        }
        if let Some(teleport) = update.teleport {
            let p = teleport.destination;
            self.packet(&SyncPlayerPosition {
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
            });
        }
    }

    pub(super) fn drain(&mut self, registry: &Registry) {
        block_on(async {
            let mut bytes = 0;
            let mut frames = 0;
            while bytes < WRITE_SLICE_BYTES && frames < WRITE_SLICE_FRAMES {
                if frames <= WRITE_SLICE_FRAMES / 2
                    && let Some(frame) = self.world_observer.next_frame(&registry.arena).unwrap()
                {
                    bytes += frame.len();
                    frames += self.world_observer.frame_count();
                    self.stream.write_all(&frame).await.unwrap();
                    self.stream.flush().await.unwrap();
                    self.world_observer.sent();
                    continue;
                }
                let Some(frame) = self.observer.next_frame().unwrap() else {
                    break;
                };
                bytes += frame.len();
                frames += self.observer.next_frame_count();
                self.stream.write_all(&frame).await.unwrap();
                self.stream.flush().await.unwrap();
                self.observer.frame_sent();
            }
        });
    }
}
