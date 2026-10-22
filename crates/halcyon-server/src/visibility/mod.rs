//! Per-connection lobby presentation. Registry borrows end before transport I/O.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use mc_protocol::error::EncodeError;
use mc_protocol::packets::client::PlayerInfoRemove;
use thiserror::Error;
use uuid::Uuid;

use crate::lobby::{PlayerSnapshot, Registry, Transform};

mod delivery;
mod frame;

use frame::{encode, roster_entry, roster_frame, spawn_frame};

type Column = (i32, i32);

const MAX_BYTES: usize = 256 * 1024;
const MAX_FRAMES: usize = 2048;
const ADMISSIONS_PER_PUBLICATION: usize = 64;
/// Tab-list additions travel as multi-entry Player Info Update packets and
/// are paced separately from entity spawns.
const ROSTER_BATCH_ENTRIES: usize = 64;
const ROSTER_BATCH_BYTES: usize = 32 * 1024;
const ROSTER_BATCHES_PER_PUBLICATION: usize = 4;
pub(crate) const WRITE_SLICE_FRAMES: usize = 32;
pub(crate) const WRITE_SLICE_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum VisibilityError {
    #[error("observer output exceeded its bounded capacity")]
    Overflow,
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

#[derive(Clone)]
struct Rendered {
    transform: Transform,
    skin_parts: u8,
    revision: u64,
    sneaking: bool,
    pose: mc_protocol::packets::client::PlayerPose,
    swimming: bool,
    sprinting: bool,
    using: Option<mc_protocol::packets::client::InteractionHand>,
    health: f32,
    equipment: Rc<[mc_protocol::types::ItemStack; 6]>,
    life_generation: u64,
}

enum Delivery {
    RosterAdd(Vec<(i32, Uuid)>),
    RosterRemove(i32),
    Spawn(PlayerSnapshot),
    Remove(i32),
    Move(PlayerSnapshot),
}

impl Delivery {
    fn id(&self) -> i32 {
        match self {
            Self::RosterAdd(entries) => entries.first().map_or(0, |(id, _)| *id),
            Self::RosterRemove(entity_id) | Self::Remove(entity_id) => *entity_id,
            Self::Spawn(player) | Self::Move(player) => player.entity_id,
        }
    }
}

struct Frame {
    bytes: Rc<[u8]>,
    frames: usize,
    delivery: Delivery,
    unload_barrier: bool,
}

#[derive(Default)]
pub struct Observer {
    roster: HashMap<i32, Uuid>,
    rendered: HashMap<i32, Rendered>,
    latest: HashMap<i32, PlayerSnapshot>,
    motion_waiting: VecDeque<i32>,
    resident: HashSet<Column>,
    reliable: VecDeque<Frame>,
    priority: VecDeque<Frame>,
    active: Option<Frame>,
    pending_roster: HashSet<i32>,
    pending_spawns: HashSet<i32>,
    pending_removals: HashSet<i32>,
    pending_departures: HashSet<i32>,
    publication: Option<u64>,
    content_revision: Option<u64>,
    spatial_revision: Option<u64>,
    roster_revision: Option<u64>,
    desired: HashSet<i32>,
    candidate_ids: Vec<i32>,
    roster_waiting: VecDeque<i32>,
    spawn_waiting: VecDeque<i32>,
    admitted: HashSet<i32>,
    roster_batches: usize,
    queued_bytes: usize,
    queued_frames: usize,
    high_water_bytes: usize,
    coalesced: u64,
    sent_bytes: u64,
}

impl Observer {
    pub fn reconcile(
        &mut self,
        registry: &Registry,
        self_id: i32,
        resident: &HashSet<Column>,
    ) -> Result<(), VisibilityError> {
        let new_publication = self.publication != Some(registry.publication());
        if new_publication {
            self.publication = Some(registry.publication());
            self.admitted.clear();
            self.roster_batches = 0;
        }
        let terrain_changed = self.resident != *resident;
        let content_changed = self.content_revision != Some(registry.revision());
        let spatial_changed =
            terrain_changed || self.spatial_revision != Some(registry.spatial_revision());
        let roster_changed = self.roster_revision != Some(registry.roster_revision());
        if !content_changed
            && !spatial_changed
            && !roster_changed
            && (!new_publication
                || (self.roster_waiting.is_empty() && self.spawn_waiting.is_empty()))
        {
            return Ok(());
        }
        self.content_revision = Some(registry.revision());
        self.spatial_revision = Some(registry.spatial_revision());
        self.roster_revision = Some(registry.roster_revision());
        if terrain_changed {
            self.resident.clone_from(resident);
        }

        if spatial_changed {
            self.candidate_ids = registry
                .candidates(resident)
                .into_iter()
                .filter(|player| player.entity_id != self_id)
                .map(|player| player.entity_id)
                .collect();
            self.desired = self.candidate_ids.iter().copied().collect();
            self.cancel_where(|frame| {
                matches!(&frame.delivery, Delivery::Spawn(player)
                    if registry.get(player.entity_id).is_none_or(|current|
                        current.entity_id == self_id || !current.visible || !resident.contains(&current.transform.chunk())))
            });
            self.latest.retain(|id, _| self.desired.contains(id));
            let removals: Vec<i32> =
                self.rendered.keys().copied().filter(|id| !self.desired.contains(id)).collect();
            for id in removals {
                self.queue_remove(id, false)?;
            }
            self.spawn_waiting = self
                .candidate_ids
                .iter()
                .copied()
                .filter(|id| !self.rendered.contains_key(id) && !self.pending_spawns.contains(id))
                .collect();
        }
        if roster_changed {
            self.cancel_where(|frame| {
                matches!(&frame.delivery, Delivery::RosterAdd(entries)
                    if entries.iter().any(|(id, _)| registry.get(*id).is_none()))
            });
            let departures: Vec<(i32, Uuid)> = self
                .roster
                .iter()
                .filter(|(id, _)| registry.get(**id).is_none())
                .map(|(id, uuid)| (*id, *uuid))
                .collect();
            for (id, uuid) in departures {
                if !self.pending_departures.contains(&id) {
                    self.push(Frame {
                        bytes: encode(&PlayerInfoRemove { uuids: &[uuid] })?,
                        frames: 1,
                        delivery: Delivery::RosterRemove(id),
                        unload_barrier: false,
                    })?;
                }
            }
            let mut waiting: Vec<i32> = registry
                .players()
                .map(|player| player.entity_id)
                .filter(|id| !self.roster.contains_key(id) && !self.pending_roster.contains(id))
                .collect();
            waiting.sort_unstable();
            self.roster_waiting = waiting.into();
        }

        // Tab membership is independent of terrain and includes the local player.
        while self.roster_batches < ROSTER_BATCHES_PER_PUBLICATION {
            let mut entries = Vec::new();
            let mut bodies = Vec::new();
            let mut bytes = 0;
            while let Some(&id) = self.roster_waiting.front() {
                if entries.len() == ROSTER_BATCH_ENTRIES {
                    break;
                }
                let Some(player) = registry.get(id) else {
                    self.roster_waiting.pop_front();
                    continue;
                };
                let Some(body) = roster_entry(&player.profile.info) else {
                    // Profiles are encoded by this server; skip rather than
                    // retry a malformed one forever.
                    self.roster_waiting.pop_front();
                    continue;
                };
                if !entries.is_empty() && bytes + body.len() > ROSTER_BATCH_BYTES {
                    break;
                }
                self.roster_waiting.pop_front();
                bytes += body.len();
                entries.push((id, player.profile.uuid));
                bodies.push(body);
            }
            if entries.is_empty() {
                break;
            }
            self.roster_batches += 1;
            self.push(Frame {
                bytes: roster_frame(&bodies)?,
                frames: 1,
                delivery: Delivery::RosterAdd(entries),
                unload_barrier: false,
            })?;
        }

        // Ordinary movement looks up only the cached nearby identities. It does
        // not rebuild the full lobby roster or search spatial cells again.
        if content_changed {
            for index in 0..self.candidate_ids.len() {
                let id = self.candidate_ids[index];
                let Some(player) = registry.get(id) else { continue };
                if self.removal_pending(id) {
                    continue;
                }
                if let Some(rendered) = self.rendered.get(&id) {
                    if rendered.life_generation != player.life_generation {
                        self.queue_remove(id, false)?;
                        continue;
                    }
                    if rendered.revision != player.revision {
                        self.remember_latest(player.clone());
                    }
                    continue;
                }
                // Refresh a pending spawn rather than queueing motion against an
                // entity the transport has not delivered yet.
                if self.pending_spawns.contains(&id) {
                    let pending_index =
                        self.reliable.iter().position(|frame| match &frame.delivery {
                            Delivery::Spawn(pending) => pending.entity_id == id,
                            _ => false,
                        });
                    if let Some(index) = pending_index {
                        let pending = &self.reliable[index];
                        if let Delivery::Spawn(previous) = &pending.delivery
                            && previous.revision != player.revision
                        {
                            let frame = spawn_frame(player.clone())?;
                            self.replace(index, frame)?;
                            self.coalesced += 1;
                        }
                    } else {
                        // A selected spawn may be held for the next writer
                        // budget. Preserve newer motion until that immutable
                        // spawn is successfully delivered, then encode from
                        // its actual delivered baseline.
                        self.remember_latest(player.clone());
                    }
                    continue;
                }
            }
        }
        while let Some(&id) = self.spawn_waiting.front() {
            let Some(player) = registry.get(id) else {
                self.spawn_waiting.pop_front();
                continue;
            };
            if !self.desired.contains(&id)
                || self.rendered.contains_key(&id)
                || self.pending_spawns.contains(&id)
            {
                self.spawn_waiting.pop_front();
                continue;
            }
            if self.removal_pending(id) {
                break;
            }
            let profile_ready = self.roster.contains_key(&id) || self.pending_roster.contains(&id);
            if !profile_ready || !self.admit(id) {
                break;
            }
            self.spawn_waiting.pop_front();
            self.push(spawn_frame(player.clone())?)?;
        }
        if spatial_changed {
            self.prune_motion();
        }
        Ok(())
    }

    /// Cancel unsent work in this column and prioritize removal of delivered
    /// entities. Drain `has_unload_barrier()` before writing the chunk unload.
    pub fn before_unload(&mut self, column: Column) -> Result<(), VisibilityError> {
        self.resident.remove(&column);
        self.spatial_revision = None;
        self.cancel_where(|frame| {
            matches!(&frame.delivery, Delivery::Spawn(player) if player.transform.chunk() == column)
        });
        let removals: Vec<i32> = self
            .rendered
            .iter()
            .filter(|(id, rendered)| {
                rendered.transform.chunk() == column
                    || self.latest.get(id).is_some_and(|player| player.transform.chunk() == column)
            })
            .map(|(id, _)| *id)
            .collect();
        if self.active.as_ref().is_some_and(|frame| {
            matches!(&frame.delivery, Delivery::Spawn(player) | Delivery::Move(player)
                if player.transform.chunk() == column || removals.contains(&player.entity_id))
        }) {
            let frame = self.active.take().expect("active frame exists");
            self.queued_bytes -= frame.bytes.len();
            self.queued_frames -= frame.frames;
            self.forget_pending(&frame.delivery);
        }
        self.latest.retain(|_, player| player.transform.chunk() != column);
        for id in removals {
            self.queue_remove(id, true)?;
        }
        self.prune_motion();
        Ok(())
    }

    pub fn has_unload_barrier(&self) -> bool {
        !self.priority.is_empty() || self.active.as_ref().is_some_and(|frame| frame.unload_barrier)
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn queued_frames(&self) -> usize {
        self.queued_frames
    }

    /// Queued frames or coalesced motion still waiting for a writer budget.
    /// Motion is encoded lazily, so it is not counted in `queued_frames`.
    pub fn has_pending(&self) -> bool {
        self.queued_frames > 0 || !self.motion_waiting.is_empty()
    }

    pub fn next_frame_count(&self) -> usize {
        self.active.as_ref().map_or(0, |frame| frame.frames)
    }

    /// Delivered and not being removed, so entity-bound effects can target it.
    pub fn is_rendered(&self, id: i32) -> bool {
        self.rendered.contains_key(&id) && !self.removal_pending(id)
    }

    pub fn visible_count(&self) -> usize {
        self.rendered.len()
    }

    pub fn roster_count(&self) -> usize {
        self.roster.len()
    }

    pub fn high_water_bytes(&self) -> usize {
        self.high_water_bytes
    }

    pub fn coalesced_updates(&self) -> u64 {
        self.coalesced
    }

    pub fn sent_bytes(&self) -> u64 {
        self.sent_bytes
    }

    #[cfg(test)]
    pub(crate) fn delivery_lag(&self, registry: &Registry) -> (usize, u64) {
        let lag = self
            .rendered
            .iter()
            .filter_map(|(id, rendered)| {
                registry.get(*id).map(|player| player.revision.saturating_sub(rendered.revision))
            })
            .max()
            .unwrap_or(0);
        (self.latest.len(), lag)
    }

    #[cfg(test)]
    fn capacity_payload_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self.roster.capacity() * size_of::<(i32, Uuid)>()
            + self.rendered.capacity() * size_of::<(i32, Rendered)>()
            + self.latest.capacity() * size_of::<(i32, PlayerSnapshot)>()
            + self.resident.capacity() * size_of::<Column>()
            + self.desired.capacity() * size_of::<i32>()
            + self.candidate_ids.capacity() * size_of::<i32>()
            + (self.roster_waiting.capacity()
                + self.spawn_waiting.capacity()
                + self.motion_waiting.capacity())
                * size_of::<i32>()
            + (self.reliable.capacity() + self.priority.capacity()) * size_of::<Frame>()
            + (self.pending_roster.capacity()
                + self.pending_spawns.capacity()
                + self.pending_removals.capacity()
                + self.pending_departures.capacity()
                + self.admitted.capacity())
                * size_of::<i32>()
            + self
                .active
                .iter()
                .chain(self.reliable.iter())
                .chain(self.priority.iter())
                .filter(|frame| !matches!(frame.delivery, Delivery::RosterAdd(_)))
                .map(|frame| frame.bytes.len())
                .sum::<usize>()
    }

    fn admit(&mut self, id: i32) -> bool {
        self.admitted.contains(&id)
            || (self.admitted.len() < ADMISSIONS_PER_PUBLICATION && self.admitted.insert(id))
    }

    fn remember_latest(&mut self, player: PlayerSnapshot) {
        use std::collections::hash_map::Entry;
        match self.latest.entry(player.entity_id) {
            Entry::Vacant(entry) => {
                self.motion_waiting.push_back(player.entity_id);
                entry.insert(player);
            }
            Entry::Occupied(mut entry) => {
                if entry.get().revision != player.revision {
                    self.coalesced += 1;
                }
                entry.insert(player);
            }
        }
    }

    fn prune_motion(&mut self) {
        self.motion_waiting.retain(|id| self.latest.contains_key(id));
    }

    fn removal_pending(&self, id: i32) -> bool {
        self.pending_removals.contains(&id)
    }
}

#[cfg(test)]
#[path = "../tests/tests_visibility.rs"]
mod tests;

#[cfg(test)]
#[path = "../bench/visibility.rs"]
mod benchmarks;

#[cfg(test)]
#[path = "../bench/synthetic.rs"]
mod synthetic_benchmarks;
