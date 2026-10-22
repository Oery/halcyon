//! Authoritative player membership, spatial indexing and snapshot publication.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use mc_protocol::types::ItemStack;
use uuid::Uuid;

use super::session::Input;
use super::{
    AdmissionError, MAX_PLAYERS, PlayerSnapshot, Profile, StepMetrics, Transform, WorldAction,
};
use crate::player::PlayerState;

pub(super) struct Player {
    pub(super) snapshot: PlayerSnapshot,
    pub(super) active: bool,
    pub(super) input: Rc<Input>,
    pub(super) state: PlayerState,
}

type ColumnMove = (i32, Option<(i32, i32)>, (i32, i32));

/// Read-only access to the current published lobby state.
#[derive(Default)]
pub struct Registry {
    pub arena: crate::arena::Arena,
    pub(super) world_actions: std::collections::VecDeque<WorldAction>,
    pub(super) players: HashMap<i32, Player>,
    uuids: HashSet<Uuid>,
    pub(super) columns: HashMap<(i32, i32), HashSet<i32>>,
    pub(super) last_entity_id: i32,
    active_count: usize,
    pub(super) revision: u64,
    roster_revision: u64,
    spatial_revision: u64,
    publication: u64,
    pub(super) coalesced: u64,
    closed_scratch: Vec<i32>,
    moved_scratch: Vec<ColumnMove>,
}

impl Registry {
    #[cfg(test)]
    pub(crate) fn from_snapshots(players: Vec<PlayerSnapshot>, publication: u64) -> Self {
        let mut registry = Self {
            revision: publication,
            roster_revision: publication,
            spatial_revision: publication,
            publication,
            ..Self::default()
        };
        for snapshot in players {
            registry.last_entity_id = registry.last_entity_id.max(snapshot.entity_id);
            registry.uuids.insert(snapshot.profile.uuid);
            registry
                .columns
                .entry(snapshot.transform.chunk())
                .or_default()
                .insert(snapshot.entity_id);
            registry.players.insert(
                snapshot.entity_id,
                Player {
                    state: PlayerState::new(snapshot.transform),
                    snapshot,
                    active: true,
                    input: Rc::new(Input::new(Rc::default())),
                },
            );
        }
        registry.active_count = registry.players.len();
        registry
    }

    #[cfg(test)]
    pub(crate) fn publish_transforms(
        &mut self,
        transforms: impl IntoIterator<Item = (i32, Transform)>,
    ) {
        for (id, transform) in transforms {
            if let Some(player) = self.players.get(&id)
                && transform.is_valid()
            {
                player.input.transform.set(Some(transform));
            }
        }
        if self.publish() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Estimated owned container payload capacity and live input/profile payloads.
    /// Shared profiles are counted once per player, never once per observer.
    /// Excludes Registry itself, Rc headers, hash controls, allocator overhead,
    /// owner-task storage and sockets; this is neither RSS nor total memory.
    #[cfg(test)]
    pub(crate) fn capacity_payload_bytes(&self) -> usize {
        self.arena.capacity_payload_bytes()
            + self.players.capacity() * std::mem::size_of::<(i32, Player)>()
            + self.uuids.capacity() * std::mem::size_of::<Uuid>()
            + self.columns.capacity() * std::mem::size_of::<((i32, i32), HashSet<i32>)>()
            + self
                .columns
                .values()
                .map(|ids| ids.capacity() * std::mem::size_of::<i32>())
                .sum::<usize>()
            + self.closed_scratch.capacity() * std::mem::size_of::<i32>()
            + self.moved_scratch.capacity() * std::mem::size_of::<ColumnMove>()
            + self
                .players
                .values()
                .map(|player| {
                    std::mem::size_of::<Input>()
                        + std::mem::size_of::<Profile>()
                        + player.state.movement.capacity_payload_bytes()
                        + player.input.proposals.borrow().capacity_payload_bytes()
                        + player.input.actions.borrow().capacity_payload_bytes()
                        + std::mem::size_of::<[ItemStack; 6]>()
                        + player.snapshot.profile.username.capacity()
                        + player.snapshot.profile.info.len()
                })
                .sum::<usize>()
    }

    pub fn players(&self) -> impl Iterator<Item = &PlayerSnapshot> {
        self.players.values().filter(|player| player.active).map(|player| &player.snapshot)
    }

    pub fn get(&self, entity_id: i32) -> Option<&PlayerSnapshot> {
        self.players.get(&entity_id).filter(|player| player.active).map(|player| &player.snapshot)
    }

    pub fn candidates(&self, columns: &HashSet<(i32, i32)>) -> Vec<PlayerSnapshot> {
        let mut result = Vec::new();
        for column in columns {
            if let Some(ids) = self.columns.get(column) {
                result.extend(
                    ids.iter()
                        .filter_map(|id| self.get(*id))
                        .filter(|player| player.visible)
                        .cloned(),
                );
            }
        }
        result.sort_unstable_by_key(|player| player.entity_id);
        result
    }

    pub fn active_count(&self) -> usize {
        self.active_count
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn roster_revision(&self) -> u64 {
        self.roster_revision
    }

    pub fn spatial_revision(&self) -> u64 {
        self.spatial_revision
    }

    /// Advances on every publication, including ticks without player changes.
    pub fn publication(&self) -> u64 {
        self.publication
    }

    pub(super) fn reserve(
        &mut self,
        profile: Rc<Profile>,
        transform: Transform,
        skin_parts: u8,
        input: Rc<Input>,
    ) -> Result<i32, AdmissionError> {
        if input.closed.get() {
            return Err(AdmissionError::Closed);
        }
        if !transform.is_valid() {
            return Err(AdmissionError::InvalidTransform);
        }
        if profile.username.is_empty()
            || profile.username.len() > 16
            || !profile.username.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(AdmissionError::InvalidProfile);
        }
        if self.uuids.contains(&profile.uuid) {
            return Err(AdmissionError::DuplicateUuid);
        }
        if self.players.len() >= MAX_PLAYERS {
            return Err(AdmissionError::Full);
        }
        let entity_id = self.last_entity_id.checked_add(1).ok_or(AdmissionError::IdExhausted)?;
        self.last_entity_id = entity_id;
        self.uuids.insert(profile.uuid);
        input.accepted.set(transform);
        self.players.insert(
            entity_id,
            Player {
                snapshot: PlayerSnapshot {
                    entity_id,
                    profile,
                    transform,
                    skin_parts,
                    revision: 0,
                    sneaking: false,
                    pose: Default::default(),
                    swimming: false,
                    sprinting: false,
                    health: 20.0,
                    equipment: Rc::new(std::array::from_fn(|_| ItemStack::default())),
                    life_generation: 0,
                    visible: true,
                },
                active: false,
                input,
                state: PlayerState::new(transform),
            },
        );
        Ok(entity_id)
    }

    pub(super) fn remove(&mut self, entity_id: i32) -> bool {
        let Some(player) = self.players.remove(&entity_id) else {
            return false;
        };
        self.uuids.remove(&player.snapshot.profile.uuid);
        if player.active {
            self.remove_column(player.snapshot.transform.chunk(), entity_id);
            self.active_count -= 1;
            self.roster_revision = self.roster_revision.wrapping_add(1);
            self.spatial_revision = self.spatial_revision.wrapping_add(1);
            true
        } else {
            false
        }
    }

    fn remove_column(&mut self, column: (i32, i32), entity_id: i32) {
        if let Some(ids) = self.columns.get_mut(&column) {
            ids.remove(&entity_id);
            if ids.is_empty() {
                self.columns.remove(&column);
            }
        }
    }

    pub(super) fn cleanup(&mut self) -> bool {
        self.closed_scratch.extend(
            self.players.iter().filter(|(_, player)| player.input.closed.get()).map(|(&id, _)| id),
        );
        let mut changed = false;
        while let Some(id) = self.closed_scratch.pop() {
            changed |= self.remove(id);
        }
        changed
    }

    /// Gameplay mutates authority here; publication consumes only the latest result.
    pub(super) fn simulate(
        &mut self,
        world: &halcyon_world::World,
        now: Instant,
        notices: &mut Vec<Rc<Input>>,
    ) -> StepMetrics {
        let mut metrics = StepMetrics::default();
        for _ in 0..64 {
            let Some(action) = self.world_actions.pop_front() else {
                break;
            };
            let result = self.arena.edit(world, action.pos, action.state);
            let _ = action.reply.send(result);
        }
        self.arena.step(world, &mut self.last_entity_id);
        let world = self.arena.view(world);
        for player in self.players.values_mut() {
            let input = &player.input;
            let had_teleport = player.state.life.pending_teleport().is_some();
            if let Some(id) = input.teleport_confirmation.take() {
                player.state.life.confirm(id);
            }
            let (update, movement) = player.state.advance(
                &world,
                &mut input.proposals.borrow_mut(),
                &mut input.actions.borrow_mut(),
                now,
                input.respawn_requested.replace(false),
            );
            let accepted = player.state.movement.transform();
            let old = input.accepted.replace(accepted);
            input.generation.set(player.state.life.generation);
            if had_teleport && player.state.life.pending_teleport().is_none() {
                input.activate.set(true);
            }
            input.ready.set(
                player.state.life.alive()
                    && player.state.life.pending_teleport().is_none()
                    && (player.active || input.activate.get()),
            );
            if old != accepted {
                input.transform.set(Some(accepted));
            }
            if let Some(movement) = movement {
                metrics.proposals += movement.processed as u64;
                metrics.collision_cells += movement.collision_cells as u64;
                metrics.corrections += u64::from(movement.correction);
            }
            if update.changed() || old.chunk() != accepted.chunk() {
                input.update.borrow_mut().merge(update);
                notices.push(input.clone());
            }
        }
        metrics
    }

    pub(super) fn publish(&mut self) -> bool {
        self.publish_at(Instant::now())
    }

    pub(super) fn publish_at(&mut self, now: Instant) -> bool {
        self.publication = self.publication.wrapping_add(1);
        let mut changed = false;
        // Apply all latest input once, rather than scheduling one task per packet.
        for (&id, player) in &mut self.players {
            self.coalesced = self.coalesced.saturating_add(player.input.coalesced.replace(0));
            let old_column = player.snapshot.transform.chunk();
            let mut player_changed = false;
            let sneaking = player.state.movement.sneaking();
            let pose = player.state.movement.pose();
            let swimming = player.state.movement.swimming();
            let sprinting = player.state.movement.sprinting();
            let health = player.state.life.health();
            let visible = player.state.life.visible(now);
            let generation = player.state.life.generation;
            if player.snapshot.sneaking != sneaking
                || player.snapshot.pose != pose
                || player.snapshot.swimming != swimming
                || player.snapshot.sprinting != sprinting
                || player.snapshot.health != health
                || player.snapshot.visible != visible
                || player.snapshot.life_generation != generation
                || !Rc::ptr_eq(&player.snapshot.equipment, &player.state.equipment)
            {
                if player.snapshot.visible != visible
                    || player.snapshot.life_generation != generation
                {
                    self.spatial_revision = self.spatial_revision.wrapping_add(1);
                }
                player.snapshot.sneaking = sneaking;
                player.snapshot.pose = pose;
                player.snapshot.swimming = swimming;
                player.snapshot.sprinting = sprinting;
                player.snapshot.health = health;
                player.snapshot.visible = visible;
                player.snapshot.life_generation = generation;
                player.snapshot.equipment = player.state.equipment.clone();
                player_changed = true;
            }
            if let Some(transform) = player.input.transform.take()
                && transform != player.snapshot.transform
            {
                player.snapshot.transform = transform;
                player_changed = true;
            }
            if let Some(skin_parts) = player.input.skin_parts.take()
                && skin_parts != player.snapshot.skin_parts
            {
                player.snapshot.skin_parts = skin_parts;
                player_changed = true;
            }
            let activated = player.input.activate.replace(false) && !player.active;
            if activated {
                player.active = true;
                self.active_count += 1;
                self.roster_revision = self.roster_revision.wrapping_add(1);
                player_changed = true;
            }
            let new_column = player.snapshot.transform.chunk();
            if player.active && (activated || old_column != new_column) {
                self.moved_scratch.push((id, (!activated).then_some(old_column), new_column));
            }
            if player_changed {
                player.snapshot.revision = player.snapshot.revision.wrapping_add(1);
                changed |= player.active;
            }
        }
        if !self.moved_scratch.is_empty() {
            self.spatial_revision = self.spatial_revision.wrapping_add(1);
        }
        while let Some((id, old, new)) = self.moved_scratch.pop() {
            if let Some(old) = old {
                self.remove_column(old, id);
            }
            self.columns.entry(new).or_default().insert(id);
        }
        changed
    }
}
