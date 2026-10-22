//! Bounded player/world replication and terrain delivery barriers.

use super::Client;
use crate::error::Error;
use crate::lobby::ObserverMetrics;
use crate::visibility::{WRITE_SLICE_BYTES, WRITE_SLICE_FRAMES};

impl Client {
    pub fn reconcile_players(&mut self) -> Result<(), Error> {
        if self.pending_teleport.is_some() {
            return Ok(());
        }
        let Some(session) = &self.session else { return Ok(()) };
        let id = session.entity_id();
        self.lobby
            .with_registry(|registry| self.observer.reconcile(registry, id, self.chunks.terrain()))
            .map_err(Error::from)
    }

    async fn send_player_frame(&mut self) -> Result<Option<(usize, usize)>, Error> {
        let Some(frame) = self.observer.next_frame()? else {
            return Ok(None);
        };
        let frames = self.observer.next_frame_count();
        self.send_encoded(&frame).await?;
        self.observer.frame_sent();
        Ok(Some((frame.len(), frames)))
    }

    /// Cosmetic effects go before player frames, so a hurt event precedes the
    /// death status published in the same step.
    async fn send_effects(&mut self) -> Result<(usize, usize), Error> {
        let Some(session) = &self.session else {
            return Ok((0, 0));
        };
        let id = session.entity_id();
        let mut frames = Vec::new();
        self.lobby.with_registry(|registry| {
            self.effects.collect(
                &registry.effects,
                id,
                |entity| self.observer.is_rendered(entity),
                self.chunks.terrain(),
                &mut frames,
            )
        });
        if frames.is_empty() {
            return Ok((0, 0));
        }
        // One write for the slice; each frame is already encoded and shared.
        let batch = frames.concat();
        self.send_encoded(&batch).await?;
        Ok((batch.len(), frames.len()))
    }

    async fn send_world_frame(&mut self) -> Result<Option<(usize, usize)>, Error> {
        let frame =
            self.lobby.with_registry(|registry| self.world_observer.next_frame(&registry.arena))?;
        let Some(frame) = frame else {
            return Ok(None);
        };
        let count = self.world_observer.frame_count();
        self.send_encoded(&frame).await?;
        self.world_observer.sent();
        Ok(Some((frame.len(), count)))
    }

    /// The cached base chunk must be followed by its current edits before any
    /// entity admission. A resent center chunk resets the delivered baseline too.
    pub async fn send_terrain_overlay(&mut self, column: (i32, i32)) -> Result<(), Error> {
        self.world_observer.base_sent(column);
        loop {
            let frame = self.lobby.with_registry(|registry| {
                self.world_observer.patch_column(&registry.arena, column)
            })?;
            let Some(frame) = frame else {
                break;
            };
            self.send_encoded(&frame).await?;
            self.world_observer.sent();
            glommio::executor().yield_now().await;
        }
        Ok(())
    }

    pub async fn resync_blocks(
        &mut self,
        cells: impl IntoIterator<Item = crate::arena::BlockPos>,
    ) -> Result<(), Error> {
        use halcyon_world::BlockView;
        use mc_protocol::packets::client::BlockUpdate;
        let cells: Vec<_> = cells.into_iter().take(64).collect();
        let mut frames = Vec::new();
        self.lobby.with_registry(|registry| {
            let world = registry.arena.view(&self.server.lobby);
            for pos in cells {
                if !self.chunks.terrain().contains(&crate::arena::column(pos)) {
                    continue;
                }
                if let Some(state) = world.block_at(pos[0], pos[1], pos[2]) {
                    frames.push(BlockUpdate {
                        location: crate::arena::pack(pos),
                        block_state: i32::from(state),
                    });
                }
            }
        });
        for (index, frame) in frames.iter().enumerate() {
            self.send(frame).await?;
            if index % 32 == 31 {
                glommio::executor().yield_now().await;
            }
        }
        Ok(())
    }

    fn report_observer(&self) {
        if let Some(session) = &self.session {
            session.record_observer(ObserverMetrics {
                roster: self.observer.roster_count(),
                visible: self.observer.visible_count(),
                queued_bytes: self.observer.queued_bytes(),
                queued_frames: self.observer.queued_frames(),
                high_water_bytes: self.observer.high_water_bytes(),
                coalesced: self.observer.coalesced_updates(),
                sent_bytes: self.observer.sent_bytes(),
            });
        }
    }

    pub(super) async fn drain_players(&mut self) -> Result<(), Error> {
        self.reconcile_players()?;
        self.lobby.with_registry(|registry| {
            self.world_observer.reconcile(&registry.arena, self.chunks.terrain())
        });
        let (mut bytes, mut frames) = self.send_effects().await?;
        while frames < WRITE_SLICE_FRAMES && bytes < WRITE_SLICE_BYTES {
            // Share the connection budget fairly with world changes. World
            // frames contain at most sixteen patches or two entity packets.
            if frames < WRITE_SLICE_FRAMES / 2
                && let Some((size, count)) = self.send_world_frame().await?
            {
                bytes += size;
                frames += count;
                if bytes >= WRITE_SLICE_BYTES {
                    break;
                }
                continue;
            }
            let Some((size, count)) = self.send_player_frame().await? else {
                if frames <= WRITE_SLICE_FRAMES / 2
                    && let Some((size, count)) = self.send_world_frame().await?
                {
                    bytes += size;
                    frames += count;
                    continue;
                }
                break;
            };
            bytes += size;
            frames += count;
            if bytes >= WRITE_SLICE_BYTES {
                break;
            }
        }
        self.report_observer();
        if frames >= WRITE_SLICE_FRAMES || bytes >= WRITE_SLICE_BYTES {
            glommio::executor().yield_now().await;
        }
        Ok(())
    }

    /// The single connection writer commits removals before their chunk unload.
    pub async fn remove_players_before_unload(&mut self, column: (i32, i32)) -> Result<(), Error> {
        let ids = self.world_observer.unload_ids(column);
        for ids in ids.chunks(32) {
            self.send(&mc_protocol::packets::client::RemoveEntities { entity_ids: ids }).await?;
            glommio::executor().yield_now().await;
        }
        self.world_observer.unloaded(column);
        self.observer.before_unload(column)?;
        let mut frames = 0;
        while self.observer.has_unload_barrier() {
            if self.send_player_frame().await?.is_none() {
                return Err(Error::UnexpectedResponse);
            }
            frames += 1;
            if frames % 32 == 0 {
                glommio::executor().yield_now().await;
            }
        }
        self.report_observer();
        Ok(())
    }
}
