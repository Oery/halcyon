//! Per-client residency and a bounded, feedback-driven chunk queue. Reconciliation
//! reuses its maps/vectors and queues only missing or changed representations.
use crate::{Client, error::Error};
use mc_protocol::packets::client::{
    ChunkBatchFinished, ChunkBatchStart, SetCenterChunk, UnloadChunk,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_VIEW_DISTANCE: i32 = 8;
const MAX_BATCH: usize = 8;
const BATCH_INTERVAL: Duration = Duration::from_millis(50);
const BATCH_TIMEOUT: Duration = Duration::from_secs(30);
type Position = (i32, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency {
    Terrain,
    Padding,
}

pub struct ChunkTracker {
    sent: HashMap<Position, Residency>,
    wanted: HashMap<Position, Residency>,
    pending: Vec<(Position, Residency)>,
    gone: Vec<Position>,
    center: Option<Position>,
    center_dirty: bool,
    radius: i32,
    waiting: Option<Instant>,
    next_batch: Instant,
    batch_size: usize,
    batch_interval: Duration,
}

impl ChunkTracker {
    pub fn new(now: Instant) -> Self {
        Self {
            sent: HashMap::new(),
            wanted: HashMap::new(),
            pending: Vec::new(),
            gone: Vec::new(),
            center: None,
            center_dirty: false,
            radius: MAX_VIEW_DISTANCE,
            waiting: None,
            next_batch: now,
            batch_size: 4,
            batch_interval: BATCH_INTERVAL,
        }
    }

    pub fn set_view_distance(&mut self, distance: u8) {
        self.radius = i32::from(distance).clamp(2, MAX_VIEW_DISTANCE);
    }

    pub fn reconcile(&mut self, world: &halcyon_world::World, center: Position) {
        self.center_dirty |= self.center != Some(center);
        self.center = Some(center);
        self.wanted.clear();
        let (cx, cz) = center;
        let r = self.radius;
        for dz in -r..=r {
            for dx in -r..=r {
                let pos = (cx + dx, cz + dz);
                if world.get(pos.0, pos.1).is_some() {
                    self.wanted.insert(pos, Residency::Terrain);
                }
            }
        }
        // Include the neighbors of terrain, including holes in sparse maps.
        // Terrain outside the view is padding until it becomes visible.
        for dz in -r - 1..=r + 1 {
            for dx in -r - 1..=r + 1 {
                let pos = (cx + dx, cz + dz);
                if self.wanted.contains_key(&pos) {
                    continue;
                }
                if (-1..=1).any(|oz| {
                    (-1..=1).any(|ox| {
                        self.wanted.get(&(pos.0 + ox, pos.1 + oz)) == Some(&Residency::Terrain)
                    })
                }) {
                    self.wanted.insert(pos, Residency::Padding);
                }
            }
        }
        self.gone.clear();
        self.gone.extend(self.sent.keys().filter(|pos| !self.wanted.contains_key(pos)).copied());
        self.pending.clear();
        self.pending.extend(
            self.wanted
                .iter()
                .filter(|(pos, kind)| self.sent.get(pos) != Some(kind))
                .map(|(&pos, &kind)| (pos, kind)),
        );
        // Pop nearest first, with deterministic ties and no stable-sort scratch.
        self.pending.sort_unstable_by_key(|&((x, z), _)| {
            std::cmp::Reverse(((x - cx).pow(2) + (z - cz).pow(2), x, z))
        });
    }

    pub fn acknowledge(&mut self, chunks_per_tick: f32, now: Instant) -> Result<(), Error> {
        if !chunks_per_tick.is_finite() || chunks_per_tick <= 0.0 || self.waiting.is_none() {
            return Err(Error::UnexpectedResponse);
        }
        let sent = self.waiting.take().ok_or(Error::UnexpectedResponse)?;
        let rate = chunks_per_tick.clamp(0.01, MAX_BATCH as f32);
        self.batch_size = (rate.floor() as usize).clamp(1, MAX_BATCH);
        self.batch_interval = BATCH_INTERVAL.mul_f64(1.0 / f64::from(rate.min(1.0)));
        self.next_batch = (sent + self.batch_interval).max(now);
        Ok(())
    }

    pub fn deadline(&self) -> Option<Instant> {
        if let Some(sent) = self.waiting {
            Some(sent + BATCH_TIMEOUT)
        } else if self.center_dirty || !self.gone.is_empty() {
            Some(Instant::now())
        } else if !self.pending.is_empty() {
            Some(self.next_batch)
        } else {
            None
        }
    }
}

pub async fn send_pending(client: &mut Client) -> Result<(), Error> {
    let now = Instant::now();
    if let Some(sent) = client.chunks.waiting
        && now >= sent + BATCH_TIMEOUT
    {
        return Err(Error::Timeout);
    }
    // Unload/update the center even when no new terrain exists at this location.
    while let Some(pos) = client.chunks.gone.pop() {
        client.send(&UnloadChunk { chunk_x: pos.0, chunk_z: pos.1 }).await?;
        client.chunks.sent.remove(&pos);
        glommio::executor().yield_if_needed().await;
    }
    if client.chunks.center_dirty {
        if let Some((x, z)) = client.chunks.center {
            client.send(&SetCenterChunk { chunk_x: x, chunk_z: z }).await?;
        }
        client.chunks.center_dirty = false;
    }
    if client.chunks.waiting.is_some()
        || now < client.chunks.next_batch
        || client.chunks.pending.is_empty()
    {
        return Ok(());
    }
    let count = client.chunks.batch_size.min(client.chunks.pending.len());
    client.send(&ChunkBatchStart).await?;
    let server = std::sync::Arc::clone(&client.server);
    for _ in 0..count {
        let Some((pos, kind)) = client.chunks.pending.pop() else {
            break;
        };
        let cache = &server.packets;
        match kind {
            Residency::Terrain => {
                let frame = cache.chunks.get(&pos).ok_or(Error::InvalidPlayerData)?;
                client.send_chunk_parts(&frame.header, &frame.data, &cache.chunk_suffix).await?;
            }
            Residency::Padding => {
                client.packet_buffer.clear();
                client.packet_buffer.extend_from_slice(&cache.empty.header);
                let offset = cache.empty_coordinates;
                client.packet_buffer[offset..offset + 4].copy_from_slice(&pos.0.to_be_bytes());
                client.packet_buffer[offset + 4..offset + 8].copy_from_slice(&pos.1.to_be_bytes());
                client.send_buffered_chunk(&cache.empty.data, &cache.chunk_suffix).await?;
            }
        }
        client.chunks.sent.insert(pos, kind);
        // Ready socket writes must not let a join burst monopolize the executor.
        glommio::executor().yield_now().await;
    }
    client.send(&ChunkBatchFinished { batch_size: count as i32 }).await?;
    let sent = Instant::now();
    client.chunks.waiting = Some(sent);
    client.chunks.next_batch = sent + client.chunks.batch_interval;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcyon_world::{LoadedWorld, World, loader::LoadedChunk};

    fn world() -> World {
        World::from_loaded(LoadedWorld {
            chunks: (-4..=4)
                .map(|x| LoadedChunk { x, z: 0, data: vec![], heights: [0; 37], wire_bytes: 0 })
                .collect(),
            spawn: (0.0, 64.0, 0.0, 0.0, 0.0),
            missing_blocks: 0,
            remapped_biomes: 0,
        })
    }

    #[test]
    fn padding_is_replaced_with_terrain_and_distant_chunks_unload() {
        let world = world();
        let mut tracker = ChunkTracker::new(Instant::now());
        tracker.set_view_distance(2);
        tracker.reconcile(&world, (0, 0));
        assert_eq!(tracker.wanted.get(&(3, 0)), Some(&Residency::Padding));
        tracker.sent.clone_from(&tracker.wanted);
        tracker.reconcile(&world, (1, 0));
        assert!(tracker.pending.contains(&((3, 0), Residency::Terrain)));
        assert!(tracker.gone.contains(&(-3, 0)));
        tracker.reconcile(&world, (100, 100));
        assert!(tracker.pending.is_empty());
        assert_eq!(tracker.gone.len(), tracker.sent.len());
        assert!(tracker.center_dirty);
    }

    #[test]
    fn chunk_feedback_is_bounded_and_requires_an_outstanding_batch() {
        let now = Instant::now();
        let mut tracker = ChunkTracker::new(now);
        assert!(tracker.acknowledge(4.0, now).is_err());
        tracker.waiting = Some(now);
        assert!(tracker.acknowledge(f32::NAN, now).is_err());
        tracker.acknowledge(1000.0, now).unwrap();
        assert_eq!(tracker.batch_size, MAX_BATCH);
        tracker.waiting = Some(now);
        tracker.acknowledge(0.1, now).unwrap();
        assert_eq!(tracker.batch_size, 1);
        assert!(tracker.next_batch >= now + Duration::from_millis(499));
        tracker.set_view_distance(255);
        assert_eq!(tracker.radius, MAX_VIEW_DISTANCE);
    }
}
