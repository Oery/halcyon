//! Per-connection lobby presentation. Registry borrows end before transport I/O.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use futures_lite::future::block_on;
use mc_protocol::error::EncodeError;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::{
    EntityPositionSync, EntityStatus, MoveEntityPosition, MoveEntityPositionRotation,
    MoveEntityRotation, PlayerInfoRemove, PlayerStateMetadata, RemoveEntities, SetEquipment,
    SetHeadRotation, SpawnPlayer, encode_angle,
};
use thiserror::Error;
use uuid::Uuid;

use crate::lobby::{PlayerSnapshot, Registry, Transform};

type Column = (i32, i32);

const MAX_BYTES: usize = 256 * 1024;
const MAX_FRAMES: usize = 2048;
const ADMISSIONS_PER_PUBLICATION: usize = 16;
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
    sprinting: bool,
    health: f32,
    equipment: Rc<[mc_protocol::types::ItemStack; 6]>,
    life_generation: u64,
}

enum Delivery {
    RosterAdd { entity_id: i32, uuid: Uuid },
    RosterRemove(i32),
    Spawn(PlayerSnapshot),
    Remove(i32),
    Move(PlayerSnapshot),
}

impl Delivery {
    fn id(&self) -> i32 {
        match self {
            Self::RosterAdd { entity_id, .. }
            | Self::RosterRemove(entity_id)
            | Self::Remove(entity_id) => *entity_id,
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
                matches!(&frame.delivery, Delivery::RosterAdd { entity_id, .. }
                    if registry.get(*entity_id).is_none())
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
        while let Some(&id) = self.roster_waiting.front() {
            let Some(player) = registry.get(id) else {
                self.roster_waiting.pop_front();
                continue;
            };
            if !self.admit(id) {
                break;
            }
            self.roster_waiting.pop_front();
            self.push(Frame {
                bytes: Rc::clone(&player.profile.info),
                frames: 1,
                delivery: Delivery::RosterAdd { entity_id: id, uuid: player.profile.uuid },
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

    /// Calling this again retries the same frame without advancing its baseline.
    /// Reconciliation/unload may cancel an admission that has not been written;
    /// callers must not reconcile while a transport write is in progress.
    pub fn next_frame(&mut self) -> Result<Option<Rc<[u8]>>, VisibilityError> {
        if let Some(frame) = &self.active {
            return Ok(Some(Rc::clone(&frame.bytes)));
        }
        self.active = self.priority.pop_front().or_else(|| self.reliable.pop_front());
        while self.active.is_none() {
            let Some(id) = self.motion_waiting.pop_front() else {
                return Ok(None);
            };
            let Some(player) = self.latest.remove(&id) else {
                continue;
            };
            if self.removal_pending(id) {
                continue;
            }
            let Some(previous) = self.rendered.get(&id) else {
                continue;
            };
            if previous.life_generation != player.life_generation {
                self.queue_remove(id, false)?;
                self.active = self.priority.pop_front().or_else(|| self.reliable.pop_front());
                continue;
            }
            let frame = movement_frame(player.clone(), previous)?;
            if let Some(frame) = frame {
                self.reserve(frame.bytes.len(), frame.frames)?;
                self.active = Some(frame);
            } else {
                // A revision can change without changing a quantized wire value.
                self.rendered.insert(id, rendered(&player));
            }
        }
        Ok(self.active.as_ref().map(|frame| Rc::clone(&frame.bytes)))
    }

    /// Commit only after the entire returned buffer was written and flushed.
    pub fn frame_sent(&mut self) {
        let Some(frame) = self.active.take() else { return };
        self.queued_bytes -= frame.bytes.len();
        self.queued_frames -= frame.frames;
        self.sent_bytes += frame.bytes.len() as u64;
        self.forget_pending(&frame.delivery);
        match frame.delivery {
            Delivery::RosterAdd { entity_id, uuid } => {
                self.roster.insert(entity_id, uuid);
            }
            Delivery::RosterRemove(id) => {
                self.roster.remove(&id);
            }
            Delivery::Spawn(player) | Delivery::Move(player) => {
                self.rendered.insert(player.entity_id, rendered(&player));
            }
            Delivery::Remove(id) => {
                self.rendered.remove(&id);
                self.latest.remove(&id);
                if self.desired.contains(&id) && !self.spawn_waiting.contains(&id) {
                    self.spawn_waiting.push_back(id);
                }
            }
        }
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

    pub fn next_frame_count(&self) -> usize {
        self.active.as_ref().map_or(0, |frame| frame.frames)
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
                .filter(|frame| !matches!(frame.delivery, Delivery::RosterAdd { .. }))
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

    fn reserve(&mut self, bytes: usize, frames: usize) -> Result<(), VisibilityError> {
        if bytes > MAX_BYTES - self.queued_bytes || frames > MAX_FRAMES - self.queued_frames {
            return Err(VisibilityError::Overflow);
        }
        self.queued_bytes += bytes;
        self.queued_frames += frames;
        self.high_water_bytes = self.high_water_bytes.max(self.queued_bytes);
        Ok(())
    }

    fn push(&mut self, frame: Frame) -> Result<(), VisibilityError> {
        self.reserve(frame.bytes.len(), frame.frames)?;
        let id = frame.delivery.id();
        match frame.delivery {
            Delivery::RosterAdd { .. } => {
                self.pending_roster.insert(id);
            }
            Delivery::RosterRemove(_) => {
                self.pending_departures.insert(id);
            }
            Delivery::Spawn(_) => {
                self.pending_spawns.insert(id);
            }
            Delivery::Remove(_) => {
                self.pending_removals.insert(id);
            }
            Delivery::Move(_) => {}
        }
        if frame.unload_barrier {
            self.priority.push_back(frame);
        } else {
            self.reliable.push_back(frame);
        }
        Ok(())
    }

    fn replace(&mut self, index: usize, frame: Frame) -> Result<(), VisibilityError> {
        let previous = &self.reliable[index];
        let bytes = self.queued_bytes - previous.bytes.len();
        let frames = self.queued_frames - previous.frames;
        if frame.bytes.len() > MAX_BYTES - bytes || frame.frames > MAX_FRAMES - frames {
            return Err(VisibilityError::Overflow);
        }
        self.queued_bytes = bytes + frame.bytes.len();
        self.queued_frames = frames + frame.frames;
        self.high_water_bytes = self.high_water_bytes.max(self.queued_bytes);
        self.reliable[index] = frame;
        Ok(())
    }

    fn cancel_where(&mut self, predicate: impl Fn(&Frame) -> bool) {
        if self.active.as_ref().is_some_and(&predicate) {
            let frame = self.active.take().expect("active frame exists");
            self.queued_bytes -= frame.bytes.len();
            self.queued_frames -= frame.frames;
            self.forget_pending(&frame.delivery);
        }
        let mut index = 0;
        while index < self.reliable.len() {
            if predicate(&self.reliable[index]) {
                let frame = self.reliable.remove(index).expect("queued frame exists");
                self.queued_bytes -= frame.bytes.len();
                self.queued_frames -= frame.frames;
                self.forget_pending(&frame.delivery);
            } else {
                index += 1;
            }
        }
    }

    fn forget_pending(&mut self, delivery: &Delivery) {
        let id = delivery.id();
        match delivery {
            Delivery::RosterAdd { .. } => {
                self.pending_roster.remove(&id);
            }
            Delivery::RosterRemove(_) => {
                self.pending_departures.remove(&id);
            }
            Delivery::Spawn(_) => {
                self.pending_spawns.remove(&id);
            }
            Delivery::Remove(_) => {
                self.pending_removals.remove(&id);
            }
            Delivery::Move(_) => {}
        }
    }

    fn queue_remove(&mut self, id: i32, barrier: bool) -> Result<(), VisibilityError> {
        self.latest.remove(&id);
        if self.removal_pending(id) {
            if barrier
                && let Some(index) = self.reliable.iter().position(
                    |frame| matches!(frame.delivery, Delivery::Remove(other) if other == id),
                )
            {
                let mut frame = self.reliable.remove(index).expect("queued removal exists");
                frame.unload_barrier = true;
                self.priority.push_back(frame);
            }
            return Ok(());
        }
        self.push(Frame {
            bytes: encode(&RemoveEntities { entity_ids: &[id] })?,
            frames: 1,
            delivery: Delivery::Remove(id),
            unload_barrier: barrier,
        })
    }
}

fn rendered(player: &PlayerSnapshot) -> Rendered {
    Rendered {
        transform: player.transform,
        skin_parts: player.skin_parts,
        revision: player.revision,
        sneaking: player.sneaking,
        sprinting: player.sprinting,
        health: player.health,
        equipment: player.equipment.clone(),
        life_generation: player.life_generation,
    }
}

fn encode<'p>(packet: &impl Payload<'p>) -> Result<Rc<[u8]>, EncodeError> {
    let mut bytes = Vec::new();
    append(packet, &mut bytes)?;
    Ok(Rc::from(bytes))
}

fn append<'p>(packet: &impl Payload<'p>, bytes: &mut Vec<u8>) -> Result<(), EncodeError> {
    // Vec's AsyncWrite is immediately ready; this cannot suspend an executor.
    block_on(packet.write_packet(bytes))
}

fn spawn_frame(player: PlayerSnapshot) -> Result<Frame, EncodeError> {
    let p = player.transform;
    let mut bytes = Vec::with_capacity(96);
    append(
        &SpawnPlayer {
            entity_id: player.entity_id,
            uuid: player.profile.uuid,
            x: p.x,
            y: p.y,
            z: p.z,
            yaw: p.yaw,
            pitch: p.pitch,
        },
        &mut bytes,
    )?;
    append(&metadata(&player), &mut bytes)?;
    append(&SetHeadRotation { entity_id: player.entity_id, yaw: p.yaw }, &mut bytes)?;
    let mut frames = 3;
    if player.equipment.iter().any(|stack| !stack.is_empty()) {
        append(
            &SetEquipment { entity_id: player.entity_id, slots: &player.equipment },
            &mut bytes,
        )?;
        frames += 1;
    }
    Ok(Frame {
        bytes: Rc::from(bytes),
        frames,
        delivery: Delivery::Spawn(player),
        unload_barrier: false,
    })
}

fn movement_frame(
    player: PlayerSnapshot,
    previous: &Rendered,
) -> Result<Option<Frame>, EncodeError> {
    let id = player.entity_id;
    let p = player.transform;
    let old = previous.transform;
    let changed_position = p.x != old.x || p.y != old.y || p.z != old.z;
    let changed_rotation = encode_angle(p.yaw) != encode_angle(old.yaw)
        || encode_angle(p.pitch) != encode_angle(old.pitch);
    let mut bytes = Vec::with_capacity(96);
    let mut frames = 0;
    if changed_position {
        let deltas = [
            quantize_position(p.x) - quantize_position(old.x),
            quantize_position(p.y) - quantize_position(old.y),
            quantize_position(p.z) - quantize_position(old.z),
        ];
        if deltas.iter().all(|delta| *delta >= i16::MIN as f64 && *delta <= i16::MAX as f64) {
            let [dx, dy, dz] = [deltas[0] as i16, deltas[1] as i16, deltas[2] as i16];
            if changed_rotation {
                append(
                    &MoveEntityPositionRotation {
                        entity_id: id,
                        dx,
                        dy,
                        dz,
                        yaw: p.yaw,
                        pitch: p.pitch,
                        on_ground: p.on_ground,
                    },
                    &mut bytes,
                )?;
            } else {
                append(
                    &MoveEntityPosition { entity_id: id, dx, dy, dz, on_ground: p.on_ground },
                    &mut bytes,
                )?;
            }
        } else {
            append(
                &EntityPositionSync {
                    entity_id: id,
                    x: p.x,
                    y: p.y,
                    z: p.z,
                    yaw: p.yaw,
                    pitch: p.pitch,
                    on_ground: p.on_ground,
                },
                &mut bytes,
            )?;
        }
        frames += 1;
    } else if changed_rotation || p.on_ground != old.on_ground {
        append(
            &MoveEntityRotation {
                entity_id: id,
                yaw: p.yaw,
                pitch: p.pitch,
                on_ground: p.on_ground,
            },
            &mut bytes,
        )?;
        frames += 1;
    }
    if encode_angle(p.yaw) != encode_angle(old.yaw) {
        append(&SetHeadRotation { entity_id: id, yaw: p.yaw }, &mut bytes)?;
        frames += 1;
    }
    if player.skin_parts != previous.skin_parts
        || player.sneaking != previous.sneaking
        || player.sprinting != previous.sprinting
        || player.health != previous.health
    {
        append(&metadata(&player), &mut bytes)?;
        frames += 1;
    }
    if player.health <= 0.0 && previous.health > 0.0 {
        append(&EntityStatus { entity_id: id, status: 3 }, &mut bytes)?;
        frames += 1;
    }
    if !Rc::ptr_eq(&player.equipment, &previous.equipment) && player.equipment != previous.equipment
    {
        append(&SetEquipment { entity_id: id, slots: &player.equipment }, &mut bytes)?;
        frames += 1;
    }
    Ok((frames != 0).then(|| Frame {
        bytes: Rc::from(bytes),
        frames,
        delivery: Delivery::Move(player),
        unload_barrier: false,
    }))
}

fn metadata(player: &PlayerSnapshot) -> PlayerStateMetadata {
    PlayerStateMetadata {
        entity_id: player.entity_id,
        skin_parts: player.skin_parts,
        sneaking: player.sneaking,
        sprinting: player.sprinting,
        health: player.health,
    }
}

fn quantize_position(value: f64) -> f64 {
    // VecDeltaCodec uses Java Math.round, whose negative ties round upward.
    (value * 4096.0 + 0.5).floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lobby::Profile;
    use mc_protocol::packets::PacketWriter;
    use mc_protocol::packets::client::PlayerInfoUpdate;
    use mc_protocol::types::VarInt;

    pub(super) fn player(id: i32, x: f64, revision: u64) -> PlayerSnapshot {
        let username = format!("player{id}");
        let uuid = Uuid::from_u128(id as u128 + 1);
        let info = encode(&PlayerInfoUpdate { uuid, username: &username, properties: Vec::new() })
            .unwrap();
        PlayerSnapshot {
            entity_id: id,
            profile: Rc::new(Profile { uuid, username, info }),
            transform: Transform { x, y: 64.0, z: 1.0, ..Transform::default() },
            skin_parts: 0x7f,
            sneaking: false,
            sprinting: false,
            health: 20.0,
            equipment: Rc::new(std::array::from_fn(|_| mc_protocol::types::ItemStack::default())),
            life_generation: 0,
            visible: true,
            revision,
        }
    }

    fn packets(mut bytes: &[u8]) -> Vec<(i32, Vec<u8>)> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let length = VarInt::read(&mut bytes).unwrap().0 as usize;
            let mut payload = &bytes[..length];
            let id = VarInt::read(&mut payload).unwrap().0;
            out.push((id, payload.to_vec()));
            bytes = &bytes[length..];
        }
        out
    }

    fn drain(observer: &mut Observer) -> Vec<(i32, Vec<u8>)> {
        let mut out = Vec::new();
        while let Some(bytes) = observer.next_frame().unwrap() {
            out.extend(packets(&bytes));
            observer.frame_sent();
        }
        assert_eq!(observer.queued_bytes(), 0);
        assert_eq!(observer.queued_frames(), 0);
        out
    }

    #[test]
    fn tab_membership_terrain_visibility_and_unload_order() {
        let local = player(0, 1.0, 1);
        let near = player(1, 2.0, 1);
        let far = player(2, 160.0, 1);
        let registry = Registry::from_snapshots(vec![local.clone(), near.clone(), far.clone()], 1);
        let terrain = HashSet::from([(0, 0)]);
        let mut observer = Observer::default();
        observer.reconcile(&registry, 0, &terrain).unwrap();
        let delivered = drain(&mut observer);
        let profile_index =
            delivered.iter().rposition(|(id, _)| *id == PlayerInfoUpdate::ID.0).unwrap();
        let spawn_index = delivered.iter().position(|(id, _)| *id == SpawnPlayer::ID.0).unwrap();
        assert!(profile_index < spawn_index);
        assert_eq!(observer.roster_count(), 3);
        assert_eq!(observer.visible_count(), 1);

        observer.before_unload((0, 0)).unwrap();
        assert!(observer.has_unload_barrier());
        let removal = observer.next_frame().unwrap().unwrap();
        assert_eq!(packets(&removal)[0].0, RemoveEntities::ID.0);
        observer.frame_sent();
        assert!(!observer.has_unload_barrier());
        assert_eq!(observer.visible_count(), 0);
        assert_eq!(observer.roster_count(), 3);

        let departed = Registry::from_snapshots(vec![local, far], 2);
        observer.reconcile(&departed, 0, &HashSet::new()).unwrap();
        let delivered = drain(&mut observer);
        assert_eq!(delivered.iter().filter(|(id, _)| *id == PlayerInfoRemove::ID.0).count(), 1);
        assert_eq!(observer.roster_count(), 2);
    }

    #[test]
    fn equipment_uses_delivered_baselines_and_respawn_recreates_the_entity() {
        use mc_protocol::types::ItemStack;
        let local = player(0, 1.0, 1);
        let mut near = player(1, 2.0, 1);
        near.equipment = Rc::new(std::array::from_fn(|slot| {
            if slot == 0 {
                ItemStack::plain(1, 1).unwrap()
            } else {
                ItemStack::default()
            }
        }));
        let terrain = HashSet::from([(0, 0)]);
        let mut observer = Observer::default();
        observer
            .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 1), 0, &terrain)
            .unwrap();
        assert!(drain(&mut observer).iter().any(|(id, _)| *id == SetEquipment::ID.0));
        near.equipment = Rc::new(std::array::from_fn(|slot| {
            if slot == 0 {
                ItemStack::plain(2, 1).unwrap()
            } else {
                ItemStack::default()
            }
        }));
        near.revision = 2;
        observer
            .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 2), 0, &terrain)
            .unwrap();
        let selected = observer.next_frame().unwrap().unwrap();
        near.equipment = Rc::new(std::array::from_fn(|slot| {
            if slot == 0 {
                ItemStack::plain(3, 1).unwrap()
            } else {
                ItemStack::default()
            }
        }));
        near.revision = 3;
        observer
            .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 3), 0, &terrain)
            .unwrap();
        assert_eq!(observer.next_frame().unwrap().unwrap(), selected);
        observer.frame_sent();
        assert!(drain(&mut observer).iter().any(|(id, _)| *id == SetEquipment::ID.0));
        assert_eq!(observer.rendered[&1].equipment, near.equipment);
        near.life_generation = 1;
        near.revision = 4;
        observer
            .reconcile(&Registry::from_snapshots(vec![local.clone(), near.clone()], 4), 0, &terrain)
            .unwrap();
        assert!(drain(&mut observer).iter().any(|(id, _)| *id == RemoveEntities::ID.0));
        observer.reconcile(&Registry::from_snapshots(vec![local, near], 5), 0, &terrain).unwrap();
        let packets = drain(&mut observer);
        assert!(packets.iter().any(|(id, _)| *id == SpawnPlayer::ID.0));
        assert!(!packets.iter().any(|(id, _)| *id == PlayerInfoRemove::ID.0));
        assert_eq!(observer.roster_count(), 2);
    }

    #[test]
    fn movement_coalesces_against_only_successfully_delivered_baseline() {
        let local = player(0, 1.0, 1);
        let terrain = HashSet::from([(0, 0), (2, 0)]);
        let mut observer = Observer::default();
        observer
            .reconcile(
                &Registry::from_snapshots(vec![local.clone(), player(1, 1.0, 1)], 1),
                0,
                &terrain,
            )
            .unwrap();
        drain(&mut observer);
        for revision in [2, 3] {
            observer
                .reconcile(
                    &Registry::from_snapshots(
                        vec![local.clone(), player(1, revision as f64, revision)],
                        revision,
                    ),
                    0,
                    &terrain,
                )
                .unwrap();
        }
        assert_eq!(observer.coalesced_updates(), 1);
        let bytes = observer.next_frame().unwrap().unwrap();
        assert!(Rc::ptr_eq(&bytes, &observer.next_frame().unwrap().unwrap()));
        assert_eq!(observer.rendered[&1].transform.x, 1.0);
        let encoded = packets(&bytes);
        assert_eq!(encoded[0].0, MoveEntityPosition::ID.0);
        let movement = MoveEntityPosition::decode_payload(&mut encoded[0].1.as_slice()).unwrap();
        assert_eq!(movement.dx, 2 * 4096);
        observer.frame_sent();
        assert_eq!(observer.rendered[&1].transform.x, 3.0);

        observer
            .reconcile(&Registry::from_snapshots(vec![local, player(1, 40.0, 4)], 4), 0, &terrain)
            .unwrap();
        assert_eq!(
            packets(&observer.next_frame().unwrap().unwrap())[0].0,
            EntityPositionSync::ID.0
        );
        observer.frame_sent();
        assert_eq!(observer.rendered[&1].transform.x, 40.0);

        // A writer budget can select a spawn but defer its write to a later
        // pass. New publications must survive that old spawn's commit.
        let mut staged = Observer::default();
        staged
            .reconcile(
                &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 1.0, 1)], 1),
                0,
                &terrain,
            )
            .unwrap();
        loop {
            staged.next_frame().unwrap().unwrap();
            if matches!(staged.active.as_ref().unwrap().delivery, Delivery::Spawn(_)) {
                break;
            }
            staged.frame_sent();
        }
        staged
            .reconcile(
                &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 2.0, 2)], 2),
                0,
                &terrain,
            )
            .unwrap();
        staged.frame_sent();
        let pending = packets(&staged.next_frame().unwrap().unwrap());
        let movement = MoveEntityPosition::decode_payload(&mut pending[0].1.as_slice()).unwrap();
        assert_eq!(movement.dx, 4096);
        staged.frame_sent();
        assert_eq!(staged.rendered[&1].transform.x, 2.0);
    }

    #[test]
    fn relative_positions_follow_java_rounding_at_positive_and_negative_half_units() {
        for (old, next, delta) in
            [(-0.5, 0.5, 1), (-0.51, -0.49, 1), (0.49, 0.51, 1), (0.51, 0.49, -1)]
        {
            let previous = player(1, old / 4096.0, 1);
            let current = player(1, next / 4096.0, 2);
            let frame = movement_frame(current, &rendered(&previous)).unwrap().unwrap();
            let encoded = packets(&frame.bytes);
            let movement =
                MoveEntityPosition::decode_payload(&mut encoded[0].1.as_slice()).unwrap();
            assert_eq!(movement.dx, delta, "old={old} next={next}");
        }
    }

    #[test]
    fn unpublished_spawn_and_selected_movement_are_cancelled_before_unload() {
        let local = player(0, 1.0, 1);
        let terrain = HashSet::from([(0, 0)]);
        let registry = Registry::from_snapshots(vec![local.clone(), player(1, 2.0, 1)], 1);
        let mut observer = Observer::default();
        observer.reconcile(&registry, 0, &terrain).unwrap();
        observer.before_unload((0, 0)).unwrap();
        assert!(drain(&mut observer).iter().all(|(id, _)| *id != SpawnPlayer::ID.0));
        assert_eq!(observer.visible_count(), 0);

        observer
            .reconcile(
                &Registry::from_snapshots(vec![local.clone(), player(1, 2.0, 2)], 2),
                0,
                &terrain,
            )
            .unwrap();
        drain(&mut observer);
        observer
            .reconcile(&Registry::from_snapshots(vec![local, player(1, 3.0, 3)], 3), 0, &terrain)
            .unwrap();
        assert!(observer.next_frame().unwrap().is_some());
        observer.before_unload((0, 0)).unwrap();
        let delivered = drain(&mut observer);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, RemoveEntities::ID.0);

        let mut selected = Observer::default();
        selected
            .reconcile(
                &Registry::from_snapshots(vec![player(0, 1.0, 1), player(1, 2.0, 1)], 1),
                0,
                &terrain,
            )
            .unwrap();
        loop {
            selected.next_frame().unwrap().unwrap();
            if matches!(selected.active.as_ref().unwrap().delivery, Delivery::Spawn(_)) {
                break;
            }
            selected.frame_sent();
        }
        selected
            .reconcile(&Registry::from_snapshots(vec![player(0, 1.0, 1)], 2), 0, &terrain)
            .unwrap();
        assert!(drain(&mut selected).iter().all(|(id, _)| *id != SpawnPlayer::ID.0));
        assert_eq!(selected.visible_count(), 0);
        assert_eq!(selected.roster_count(), 1);
    }

    #[test]
    fn admissions_are_paced_per_publication_and_backlog_is_bounded() {
        let records: Vec<_> = (0..40).map(|id| player(id, 1.0, 1)).collect();
        let registry = Registry::from_snapshots(records.clone(), 1);
        let mut observer = Observer::default();
        observer.reconcile(&registry, 0, &HashSet::new()).unwrap();
        drain(&mut observer);
        observer.reconcile(&registry, 0, &HashSet::new()).unwrap();
        assert!(drain(&mut observer).is_empty());
        assert_eq!(observer.roster_count(), 16);
        observer.reconcile(&Registry::from_snapshots(records, 2), 0, &HashSet::new()).unwrap();
        drain(&mut observer);
        assert_eq!(observer.roster_count(), 32);

        let mut observer = Observer::default();
        assert!(matches!(observer.reserve(MAX_BYTES + 1, 1), Err(VisibilityError::Overflow)));
        assert_eq!(observer.queued_bytes(), 0);
        observer.reserve(MAX_BYTES, MAX_FRAMES).unwrap();
        assert!(matches!(observer.reserve(0, 1), Err(VisibilityError::Overflow)));
        assert!(matches!(observer.reserve(1, 0), Err(VisibilityError::Overflow)));
        assert_eq!(observer.queued_bytes(), MAX_BYTES);
        assert_eq!(observer.queued_frames(), MAX_FRAMES);
    }

    #[test]
    fn continuously_moving_players_share_a_limited_writer_budget() {
        let players: Vec<_> = (0..80).map(|id| player(id, 1.0, 1)).collect();
        let mut registry = Registry::from_snapshots(players.clone(), 1);
        let terrain = HashSet::from([(0, 0)]);
        let mut observer = Observer::default();
        for _ in 0..6 {
            registry.publish_transforms([]);
            observer.reconcile(&registry, 0, &terrain).unwrap();
            drain(&mut observer);
        }
        assert_eq!(observer.visible_count(), 79);
        for tick in 0..3 {
            registry.publish_transforms(players.iter().map(|player| {
                let mut transform = player.transform;
                transform.x += 0.25 + tick as f64 * 0.01;
                (player.entity_id, transform)
            }));
            observer.reconcile(&registry, 0, &terrain).unwrap();
            let mut frames = 0;
            while frames < 32 && observer.next_frame().unwrap().is_some() {
                frames += observer.next_frame_count();
                observer.frame_sent();
            }
            assert_eq!(observer.motion_waiting.len(), observer.latest.len());
        }
        assert!(observer.rendered.values().all(|player| player.transform.x > 1.0));
    }

    #[test]
    #[ignore = "manual synthetic lobby fanout measurement; no sockets or gameplay actors"]
    fn synthetic_lobby() {
        use std::time::Instant;

        fn drain_sink(observer: &mut Observer) -> (usize, usize) {
            let mut bytes = 0;
            let mut frames = 0;
            while let Some(frame) = observer.next_frame().unwrap() {
                bytes += frame.len();
                frames += observer.next_frame_count();
                observer.frame_sent();
            }
            (bytes, frames)
        }

        for population in [128, 1024, 4096] {
            for dense in [false, true] {
                let players: Vec<_> = (0..population)
                    .map(|id| player(id, if dense { 1.0 } else { id as f64 * 32.0 + 1.0 }, 1))
                    .collect();
                let profile_payload: usize = players
                    .iter()
                    .map(|player| {
                        std::mem::size_of::<Profile>()
                            + player.profile.username.len()
                            + player.profile.info.len()
                    })
                    .sum();
                let mut registry = Registry::from_snapshots(players.clone(), 1);
                let mut observers: Vec<_> = (0..16)
                    .map(|id| {
                        let terrain = if dense {
                            HashSet::from([(0, 0)])
                        } else {
                            HashSet::from([(id * 2, 0), ((id + 1) * 2, 0)])
                        };
                        (Observer::default(), terrain)
                    })
                    .collect();

                // Complete the same paced admissions as production, outside
                // the measured window. No full-roster burst is manufactured.
                let admission_start = Instant::now();
                let mut admission_bytes = 0;
                for _ in 0..(population as usize).div_ceil(16) + 2 {
                    registry.publish_transforms([]);
                    for (id, (observer, terrain)) in observers.iter_mut().enumerate() {
                        observer.reconcile(&registry, id as i32, terrain).unwrap();
                        admission_bytes += drain_sink(observer).0;
                    }
                }
                let admission_us = admission_start.elapsed().as_micros();
                assert!(
                    observers
                        .iter()
                        .all(|(observer, _)| observer.roster_count() == population as usize)
                );
                let edges: usize =
                    observers.iter().map(|(observer, _)| observer.visible_count()).sum();
                assert!(edges <= 16 * 4096);
                let mut samples = [0u64; 1000];
                let mut bytes = 0usize;
                let mut frames = 0usize;
                let mut coalesced_start = 0;
                for tick in 0..1100 {
                    if tick == 100 {
                        coalesced_start = observers
                            .iter()
                            .map(|(observer, _)| observer.coalesced_updates())
                            .sum::<u64>();
                    }
                    let start = Instant::now();
                    let offset = if tick % 2 == 0 { 0.25 } else { 0.5 };
                    registry.publish_transforms(players.iter().map(|player| {
                        let mut transform = player.transform;
                        transform.x += offset;
                        transform.yaw = (tick % 360) as f32;
                        (player.entity_id, transform)
                    }));
                    for (id, (observer, terrain)) in observers.iter_mut().enumerate() {
                        observer.reconcile(&registry, id as i32, terrain).unwrap();
                        // Withhold one drain in four to exercise replacement
                        // of pending movement before it is encoded/delivered.
                        if tick % 4 != 0 {
                            let (sent_bytes, sent_frames) = drain_sink(observer);
                            if tick >= 100 {
                                bytes += sent_bytes;
                                frames += sent_frames;
                            }
                        }
                    }
                    if tick >= 100 {
                        samples[tick - 100] = start.elapsed().as_nanos() as u64;
                    }
                }
                samples.sort_unstable();
                let mean_us =
                    samples.iter().map(|sample| *sample as f64).sum::<f64>() / 1_000_000.0;
                let coalesced =
                    observers.iter().map(|(observer, _)| observer.coalesced_updates()).sum::<u64>()
                        - coalesced_start;
                let high_water = observers
                    .iter()
                    .map(|(observer, _)| observer.high_water_bytes())
                    .max()
                    .unwrap();
                let delivered_bytes: u64 =
                    observers.iter().map(|(observer, _)| observer.sent_bytes()).sum();
                let observer_capacity_payload_bytes: usize =
                    observers.iter().map(|(observer, _)| observer.capacity_payload_bytes()).sum();
                let shared_registry_capacity_payload_bytes = registry.capacity_payload_bytes();

                // A sink that never drains must either retain a bounded set
                // of coalesced state or reject further reliable admissions.
                let mut stalled = Observer::default();
                let mut overflow = false;
                for tick in 0..130 {
                    let mut transform = players[1].transform;
                    transform.x += if tick % 2 == 0 { 0.125 } else { 0.375 };
                    registry.publish_transforms([(1, transform)]);
                    let (healthy, terrain) = &mut observers[0];
                    healthy.reconcile(&registry, 0, terrain).unwrap();
                    assert!(drain_sink(healthy).1 > 0);
                    match stalled.reconcile(&registry, 0, &observers[0].1) {
                        Ok(()) => {}
                        Err(VisibilityError::Overflow) => {
                            overflow = true;
                            break;
                        }
                        Err(error) => panic!("unexpected encoding error: {error}"),
                    }
                    assert!(stalled.queued_bytes() <= MAX_BYTES);
                    assert!(stalled.queued_frames() <= MAX_FRAMES);
                }
                assert_eq!(registry.active_count(), population as usize);
                if population == 4096 {
                    assert!(overflow);
                }
                eprintln!(
                    "synthetic lobby population={population} layout={} observers=16 visible_edges={edges} admission_us={admission_us} admission_encoded_bytes={admission_bytes} ticks=1000 mean_us={mean_us:.1} p95_us={:.1} max_us={:.1} encoded_bytes={bytes} protocol_frames={frames} coalesced={coalesced} high_water_bytes={high_water} delivered_bytes_including_warmup={delivered_bytes} shared_profile_payload_bytes_per_actor={:.1} shared_registry_capacity_payload_bytes={shared_registry_capacity_payload_bytes} observer_capacity_payload_bytes={observer_capacity_payload_bytes} stalled_overflow={overflow} stalled_bytes={} stalled_frames={} (in-memory sinks; excludes sockets, encryption, chunks, shared scheduler, hash controls, Rc headers and allocator overhead)",
                    if dense { "dense" } else { "sparse" },
                    samples[949] as f64 / 1000.0,
                    samples[999] as f64 / 1000.0,
                    profile_payload as f64 / population as f64,
                    stalled.queued_bytes(),
                    stalled.queued_frames(),
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "bench/visibility.rs"]
mod benchmarks;
