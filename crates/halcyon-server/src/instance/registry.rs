//! Authoritative player membership, spatial indexing and snapshot publication.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use halcyon_world::BlockView;
use mc_protocol::packets::client::InteractionHand;
use mc_protocol::types::ItemStack;
use uuid::Uuid;

use super::session::Input;
use super::{AdmissionError, PlayerSnapshot, Profile, StepMetrics, Transform, WorldAction};
use crate::combat::{self, Armor};
use crate::effects::{Particle, Particles, Sound};
use crate::game::GameEvent;
use crate::interaction::{
    BLOCK_ACTIONS_PER_STEP, BlockInteraction, advance_mining, break_cells, execute,
};
use crate::lifecycle::{Damage, DamageKind, DamageResult};
use crate::player::{PendingAttack, PlayerAction, PlayerState, PlayerUpdate};

pub(super) struct Player {
    pub(super) snapshot: PlayerSnapshot,
    pub(super) active: bool,
    pub(super) input: Rc<Input>,
    pub(super) state: PlayerState,
}

type ColumnMove = (i32, Option<(i32, i32)>, (i32, i32));

/// Generous allowance over a spectator's three-block entity interaction range.
const SPECTATE_REACH: f64 = 8.0;
type ActionResponse =
    glommio::channels::oneshot::Receiver<Result<(), crate::player::PlayerActionError>>;

/// Read-only access to the current published lobby state.
#[derive(Default)]
pub struct Registry {
    pub arena: crate::arena::Arena,
    pub effects: crate::effects::EffectLog,
    pub(super) world_actions: std::collections::VecDeque<WorldAction>,
    pub(super) players: HashMap<i32, Player>,
    uuids: HashSet<Uuid>,
    /// Admitted entity IDs in ascending order, maintained on admission and
    /// removal so games read a stable order without sorting each step.
    pub(super) members: Vec<i32>,
    pub(super) columns: HashMap<(i32, i32), HashSet<i32>>,
    /// Shared by every instance; see `EntityIds`.
    pub(super) ids: super::EntityIds,
    active_count: usize,
    pub(super) revision: u64,
    roster_revision: u64,
    spatial_revision: u64,
    publication: u64,
    pub(super) coalesced: u64,
    closed_scratch: Vec<i32>,
    moved_scratch: Vec<ColumnMove>,
    camera_scratch: Vec<i32>,
    /// Attackers this step, and whether their movement allows a critical hit.
    attack_scratch: Vec<(i32, bool)>,
    sweep_scratch: Vec<i32>,
    /// Sound seeds and degenerate knockback directions.
    rng: crate::bot::rng::Rng,
    /// Membership and life changes for the game, drained after each step.
    pub(super) events: Vec<crate::game::GameEvent>,
}

/// A validated melee attack, computed from the attacker's and target's authority.
#[derive(Clone, Copy)]
struct Hit {
    attacker: i32,
    target: i32,
    damage: f32,
    strength: f32,
    strong: bool,
    crit: bool,
    sprint_knockback: bool,
    sweep: bool,
    pvp: bool,
    origin: Transform,
    /// The attacker's yaw as a knockback direction, `(sin, -cos)`.
    facing: [f64; 2],
    /// The attacker's body height, for its sweep particle.
    height: f64,
    target_bounds: halcyon_world::Aabb,
}

impl Registry {
    pub fn inventory_snapshot(&self, id: i32) -> Option<crate::inventory::InventorySnapshot> {
        Some(self.players.get(&id)?.state.inventory.snapshot())
    }
    pub(super) fn request_give(
        &self,
        sender: i32,
        stack: ItemStack,
    ) -> Result<
        glommio::channels::oneshot::Receiver<Result<ItemStack, crate::player::PlayerActionError>>,
        crate::player::PlayerActionError,
    > {
        let player = self.command_player(sender, None)?;
        let (reply, response) = glommio::channels::oneshot::oneshot();
        if !player
            .input
            .actions
            .borrow_mut()
            .push(player.input.generation.get(), PlayerAction::GiveCreative { stack, reply })
        {
            return Err(crate::player::PlayerActionError::QueueFull);
        }
        Ok(response)
    }
    fn command_player(
        &self,
        sender: i32,
        target: Option<&str>,
    ) -> Result<&Player, crate::player::PlayerActionError> {
        use crate::player::PlayerActionError;
        let player = if let Some(name) = target {
            let mut matches = self.players.values().filter(|player| {
                player.active
                    && !player.input.closed.get()
                    && player.snapshot.profile.username.eq_ignore_ascii_case(name)
            });
            let player = matches.next().ok_or(PlayerActionError::NotFound)?;
            if matches.next().is_some() {
                return Err(PlayerActionError::Ambiguous);
            }
            player
        } else {
            self.players.get(&sender).ok_or(PlayerActionError::NotFound)?
        };
        if player.input.closed.get() {
            return Err(PlayerActionError::Closed);
        }
        if !player.active || !player.input.ready.get() {
            return Err(PlayerActionError::NotReady);
        }
        Ok(player)
    }

    pub(super) fn request_game_mode(
        &self,
        sender: i32,
        target: Option<&str>,
        mode: mc_protocol::types::GameMode,
    ) -> Result<(String, ActionResponse), crate::player::PlayerActionError> {
        use crate::player::PlayerActionError;
        let player = self.command_player(sender, target)?;
        let (reply, response) = glommio::channels::oneshot::oneshot();
        if !player
            .input
            .actions
            .borrow_mut()
            .push(player.input.generation.get(), PlayerAction::SetGameMode { mode, reply })
        {
            return Err(PlayerActionError::QueueFull);
        }
        Ok((player.snapshot.profile.username.clone(), response))
    }

    pub fn command_player_names(&self, prefix: &str) -> Vec<String> {
        if prefix.len() > 16 || !prefix.is_ascii() {
            return Vec::new();
        }
        let mut names: Vec<_> = self
            .players
            .values()
            .filter(|player| {
                player.active && player.input.ready.get() && !player.input.closed.get()
            })
            .map(|player| &player.snapshot.profile.username)
            .filter(|name| {
                name.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            })
            .cloned()
            .collect();
        names.sort_unstable();
        names.dedup();
        names.truncate(64);
        names
    }

    pub(super) fn request_teleport(
        &self,
        sender: i32,
        target: Option<&str>,
        destination: super::TeleportDestination<'_>,
    ) -> Result<(String, Transform, ActionResponse), crate::player::PlayerActionError> {
        use crate::player::PlayerActionError;
        let player = self.command_player(sender, target)?;
        let position = match destination {
            super::TeleportDestination::Player(name) => {
                self.command_player(sender, Some(name))?.state.movement.transform()
            }
            super::TeleportDestination::Position(position) => {
                let origin = self.command_player(sender, None)?.state.movement.transform();
                let [x, y, z] = position.resolve([origin.x, origin.y, origin.z]);
                Transform { x, y, z, ..player.state.movement.transform() }
            }
        };
        if !position.is_valid() {
            return Err(PlayerActionError::InvalidDestination);
        }
        let (reply, response) = glommio::channels::oneshot::oneshot();
        if !player.input.actions.borrow_mut().push(
            player.input.generation.get(),
            PlayerAction::TeleportRequest { destination: position, reply },
        ) {
            return Err(PlayerActionError::QueueFull);
        }
        Ok((player.snapshot.profile.username.clone(), position, response))
    }

    #[cfg(test)]
    pub(crate) fn from_snapshots(players: Vec<PlayerSnapshot>, publication: u64) -> Self {
        let mut registry = Self {
            revision: publication,
            roster_revision: publication,
            spatial_revision: publication,
            publication,
            ids: super::EntityIds::starting_after(
                players.iter().map(|snapshot| snapshot.entity_id).max().unwrap_or(0),
            ),
            ..Self::default()
        };
        for snapshot in players {
            registry.uuids.insert(snapshot.profile.uuid);
            registry.admit(snapshot.entity_id);
            registry
                .columns
                .entry(snapshot.transform.chunk())
                .or_default()
                .insert(snapshot.entity_id);
            registry.players.insert(
                snapshot.entity_id,
                Player {
                    state: {
                        let mut state = PlayerState::new(snapshot.transform);
                        state.game_mode.current = snapshot.game_mode;
                        state
                    },
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
            + self.camera_scratch.capacity() * std::mem::size_of::<i32>()
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
        let entity_id = self.ids.next().ok_or(AdmissionError::IdExhausted)?;
        self.reserve_as(entity_id, profile, transform, skin_parts, input)
    }

    /// Admits a player under an ID it already holds from an earlier instance.
    pub(super) fn reserve_as(
        &mut self,
        entity_id: i32,
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
        // IDs follow their account, so a taken ID is the same player again.
        if self.uuids.contains(&profile.uuid) || self.players.contains_key(&entity_id) {
            return Err(AdmissionError::DuplicateUuid);
        }
        self.uuids.insert(profile.uuid);
        self.admit(entity_id);
        input.accepted.set(transform);
        self.players.insert(
            entity_id,
            Player {
                snapshot: PlayerSnapshot {
                    game_mode: Default::default(),
                    entity_id,
                    profile,
                    transform,
                    skin_parts,
                    revision: 0,
                    sneaking: false,
                    pose: Default::default(),
                    swimming: false,
                    sprinting: false,
                    using: None,
                    health: 20.0,
                    equipment: Rc::new(std::array::from_fn(|_| ItemStack::default())),
                    life_generation: 0,
                    visible: true,
                    camera_attached: false,
                },
                active: false,
                input,
                state: PlayerState::new(transform),
            },
        );
        Ok(entity_id)
    }

    fn admit(&mut self, entity_id: i32) {
        if let Err(index) = self.members.binary_search(&entity_id) {
            self.members.insert(index, entity_id);
        }
    }

    pub(super) fn remove(&mut self, entity_id: i32) -> bool {
        let Some(player) = self.players.remove(&entity_id) else {
            return false;
        };
        self.events.push(GameEvent::Left(entity_id));
        self.arena.cracks.remove(&entity_id);
        self.uuids.remove(&player.snapshot.profile.uuid);
        if let Ok(index) = self.members.binary_search(&entity_id) {
            self.members.remove(index);
        }
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
            let result = match action.edit {
                super::WorldEdit::Block { pos, state } => self.arena.edit(world, pos, state),
                super::WorldEdit::Sign { pos, text } => {
                    self.arena.block_entities.set_sign(pos, *text)
                }
                super::WorldEdit::Jukebox { pos, disc, play } => {
                    crate::interaction::set_jukebox(&mut self.arena, world, pos, disc, play)
                        .map(|_| ())
                }
                super::WorldEdit::Opening { pos, yaw } => {
                    crate::interaction::toggled_cells(&self.arena.view(world), pos, yaw)
                        .ok_or(crate::arena::EditError::Unsupported)
                        .and_then(|cells| self.arena.edit_all(world, &cells).map(|_| ()))
                }
            };
            let _ = action.reply.send(result);
        }
        for player in self.players.values() {
            player.input.actions.borrow_mut().barrier =
                player.input.block_actions.borrow().barrier();
        }
        self.arena.step(world, &self.ids);
        if self.arena.tick().is_multiple_of(20) {
            for pos in self.arena.block_entities.playing.keys() {
                self.effects.particles(
                    crate::effects::Particles {
                        particle: crate::effects::Particle::NOTE,
                        pos: [
                            f64::from(pos[0]) + 0.5,
                            f64::from(pos[1]) + 1.2,
                            f64::from(pos[2]) + 0.5,
                        ],
                        offset: [0.0; 3],
                        speed: 0.0,
                        count: 1,
                    },
                    None,
                );
            }
        }
        let view = self.arena.view(world);
        for (&id, player) in &mut self.players {
            let input = &player.input;
            let had_teleport = player.state.life.pending_teleport().is_some();
            if let Some(id) = input.teleport_confirmation.take() {
                player.state.life.confirm(id);
            }
            let (update, movement) = player.state.advance(
                &view,
                &mut input.proposals.borrow_mut(),
                &mut input.actions.borrow_mut(),
                now,
                input.respawn_requested.replace(false),
            );
            if let Some(death) = update.death {
                self.events.push(GameEvent::Died { player: id, killer: death.source });
            }
            if update.respawn {
                self.events.push(GameEvent::Respawned(id));
            }
            if player.active {
                if let Some(damage) = update.damage {
                    self.effects.hurt(id, damage);
                }
                let swings = input.swings.replace(0);
                if swings & (1 << InteractionHand::Main as u8) != 0 {
                    // Swinging restarts the charge after this step's attack used it.
                    player.state.reset_attack_strength();
                }
                if player.state.life.alive() && !player.state.spectator() {
                    for hand in [InteractionHand::Main, InteractionHand::Off] {
                        if swings & (1 << hand as u8) != 0 {
                            // Bots have no client; a human already saw its own swing.
                            self.effects.swing(id, hand, Some(id));
                        }
                    }
                }
            } else {
                input.swings.set(0);
            }
            if player.state.camera_pending() {
                self.camera_scratch.push(id);
            }
            if player.state.attack_pending() {
                self.attack_scratch.push((id, player.state.movement.can_critical(&view)));
            }
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
        self.resolve_attacks(now, notices);
        let occupants: Vec<_> = self
            .players
            .values()
            .filter(|p| p.active && p.state.life.alive() && !p.state.spectator())
            .map(|p| crate::interaction::body(&p.state))
            .collect();
        for (&id, player) in &mut self.players {
            let input = &player.input;
            let mut update = PlayerUpdate::default();
            if input.cancel_mining.replace(false) {
                player.state.mining = None;
                self.arena.cracks.remove(&id);
            }
            if input.update.borrow().used_blocks.len() > crate::player::MAX_USED_BLOCKS - 512 {
                crate::interaction::cancel_invalid_mining(
                    &mut self.arena,
                    world,
                    &mut player.state,
                    id,
                );
                continue;
            }
            for _ in 0..BLOCK_ACTIONS_PER_STEP {
                if input
                    .block_actions
                    .borrow()
                    .barrier()
                    .is_some_and(|barrier| barrier > input.actions.borrow().completed())
                {
                    break;
                }
                let Some((generation, action)) = input.block_actions.borrow_mut().pop() else {
                    break;
                };
                update.block_ack = update.block_ack.max(Some(action.sequence()));
                let applied = if generation != player.state.life.generation
                    || !player.active
                    || input.closed.get()
                {
                    None
                } else {
                    execute(
                        &mut self.arena,
                        world,
                        &mut player.state,
                        id,
                        action,
                        &occupants,
                        &mut update,
                    )
                };
                if let Some(applied) = applied {
                    applied.emit(&mut self.effects, id, action.hand());
                    update.used_blocks.extend(applied.cells);
                } else {
                    if matches!(action, BlockInteraction::Break(_)) {
                        update
                            .used_blocks
                            .extend(break_cells(&self.arena.view(world), action.pos()));
                    } else if !matches!(action, BlockInteraction::ItemUse { .. }) {
                        let [x, y, z] = action.pos().map(f64::from);
                        update.used_blocks.extend(crate::world_sync::resync_cells(Transform {
                            x: x + 0.5,
                            y,
                            z: z + 0.5,
                            ..Default::default()
                        }));
                    }
                    update.inventory_full = true;
                    update.inventory = Some(player.state.inventory.snapshot());
                }
            }
            let mining_target = player.state.mining.as_ref().map(|m| m.pos);
            let mined = mining_target
                .and_then(|pos| self.arena.view(world).block_at(pos[0], pos[1], pos[2]));
            if let Some(cells) = advance_mining(&mut self.arena, world, &mut player.state, id) {
                if let (Some(pos), Some(state)) = (mining_target, mined) {
                    self.effects.block_break(pos, state, Some(id));
                }
                update.used_blocks.extend(cells);
            }
            if update.changed() || !update.used_blocks.is_empty() {
                input.update.borrow_mut().merge(update);
                notices.push(input.clone());
            }
        }
        self.resolve_cameras(notices);
        metrics
    }

    /// Melee attacks read other players' authority after every player advanced.
    fn resolve_attacks(&mut self, now: Instant, notices: &mut Vec<Rc<Input>>) {
        // Resolve in entity order, independent of map iteration.
        self.attack_scratch.sort_unstable_by_key(|&(id, _)| id);
        let mut attacks = std::mem::take(&mut self.attack_scratch);
        for (id, crit) in attacks.drain(..) {
            let Some(attack) = self.players.get_mut(&id).and_then(|p| p.state.take_attack()) else {
                continue;
            };
            if let Some(hit) = self.plan_hit(id, attack, crit) {
                self.strike(hit, now, notices);
            }
        }
        // Keep the scratch capacity for the next step.
        self.attack_scratch = attacks;
    }

    /// Player.attack's checks and damage, before anything changes. Invalid,
    /// out-of-reach or unsupported attacks do nothing, as in vanilla.
    fn plan_hit(&self, id: i32, attack: PendingAttack, crit: bool) -> Option<Hit> {
        let attacker = self.players.get(&id).filter(|p| p.active && !p.input.closed.get())?;
        let state = &attacker.state;
        if !state.life.alive() || state.spectator() || state.life.pending_teleport().is_some() {
            return None;
        }
        let target = self.players.get(&attack.target).filter(|target| {
            attack.target != id
                && target.active
                && !target.input.closed.get()
                && target.state.life.alive()
                && !target.state.spectator()
        })?;
        let origin = state.movement.transform();
        let eye = [origin.x, state.movement.eye_y(), origin.z];
        let target_bounds = target.state.movement.bounds();
        let creative = state.game_mode.current == mc_protocol::types::GameMode::Creative;
        if !combat::in_reach(eye, target_bounds, creative) {
            return None;
        }
        let weapon = combat::weapon(&state.equipment[0])?;
        let strength = weapon.strength(attack.ticks);
        let strong = combat::strong(strength);
        let crit = strong && crit;
        let sprint_knockback = strong && state.movement.sprinting();
        let sweep = strong
            && !crit
            && !sprint_knockback
            && combat::sweeps(weapon, origin.on_ground, state.movement.known_movement());
        let damage = combat::scaled_damage(weapon, strength) * if crit { 1.5 } else { 1.0 };
        let bounds = state.movement.bounds();
        let [sin, cos] = crate::movement::sin_cos(origin.yaw);
        Some(Hit {
            attacker: id,
            target: attack.target,
            damage,
            strength,
            strong,
            crit,
            sprint_knockback,
            sweep,
            pvp: state.life.rules.pvp,
            origin,
            facing: [sin, -cos],
            height: bounds.max[1] - bounds.min[1],
            target_bounds,
        })
    }

    fn strike(&mut self, hit: Hit, now: Instant, notices: &mut Vec<Rc<Input>>) {
        let id = hit.attacker;
        let origin = hit.origin;
        let pos = [origin.x, origin.y, origin.z];
        if let Some(attacker) = self.players.get_mut(&id) {
            attacker.state.reset_attack_strength();
        }
        if hit.sprint_knockback {
            self.attack_sound(Sound::ATTACK_KNOCKBACK, pos);
        }
        let extra = hit.sprint_knockback.then_some(combat::SPRINT_KNOCKBACK);
        let Some(dealt) = self.melee_damage(hit, hit.target, hit.damage, extra, now, notices)
        else {
            self.attack_sound(Sound::ATTACK_NODAMAGE, pos);
            return;
        };
        if hit.sprint_knockback
            && let Some(attacker) = self.players.get_mut(&id)
        {
            attacker.state.movement.stop_sprinting();
            if attacker.state.driven() {
                let velocity = &mut attacker.state.movement.motion.velocity;
                velocity[0] *= 0.6;
                velocity[2] *= 0.6;
            }
        }
        if hit.sweep {
            self.sweep(hit, now, notices);
        }
        if hit.crit {
            self.attack_sound(Sound::ATTACK_CRIT, pos);
            self.effects
                .animation(hit.target, mc_protocol::packets::client::animation::CRITICAL_HIT);
        } else if !hit.sweep {
            let sound = if hit.strong {
                Sound::ATTACK_STRONG
            } else {
                Sound::ATTACK_WEAK
            };
            self.attack_sound(sound, pos);
        }
        if dealt > 2.0 {
            let bounds = hit.target_bounds;
            let center = [
                (bounds.min[0] + bounds.max[0]) / 2.0,
                (bounds.min[1] + bounds.max[1]) / 2.0,
                (bounds.min[2] + bounds.max[2]) / 2.0,
            ];
            self.effects.particles(
                Particles {
                    particle: Particle::DAMAGE_INDICATOR,
                    pos: center,
                    offset: [0.1, 0.0, 0.1],
                    speed: 0.2,
                    count: (f64::from(dealt) * 0.5) as i32,
                },
                None,
            );
        }
    }

    /// `doSweepAttack`: other players around the target take a weak hit.
    fn sweep(&mut self, hit: Hit, now: Instant, notices: &mut Vec<Rc<Input>>) {
        let (id, origin) = (hit.attacker, hit.origin);
        self.attack_sound(Sound::ATTACK_SWEEP, [origin.x, origin.y, origin.z]);
        let mut area = hit.target_bounds;
        for (axis, amount) in [1.0, 0.25, 1.0].into_iter().enumerate() {
            area.min[axis] -= amount;
            area.max[axis] += amount;
        }
        let low = Transform { x: area.min[0], z: area.min[2], ..origin }.chunk();
        let high = Transform { x: area.max[0], z: area.max[2], ..origin }.chunk();
        self.sweep_scratch.clear();
        // Columns are refreshed at publication; a chunk of margin covers this step's moves.
        for x in low.0 - 1..=high.0 + 1 {
            for z in low.1 - 1..=high.1 + 1 {
                let Some(members) = self.columns.get(&(x, z)) else {
                    continue;
                };
                for &other in members {
                    let Some(player) = self.players.get(&other) else {
                        continue;
                    };
                    let transform = player.state.movement.transform();
                    let [dx, dy, dz] =
                        [transform.x - origin.x, transform.y - origin.y, transform.z - origin.z];
                    if other != id
                        && other != hit.target
                        && player.active
                        && !player.input.closed.get()
                        && player.state.life.alive()
                        && !player.state.spectator()
                        && dx * dx + dy * dy + dz * dz < combat::SWEEP_RANGE_SQR
                        && combat::intersects(area, player.state.movement.bounds())
                    {
                        self.sweep_scratch.push(other);
                    }
                }
            }
        }
        self.sweep_scratch.sort_unstable();
        // Without Sweeping Edge, each swept player takes the charge-scaled minimum.
        for index in 0..self.sweep_scratch.len() {
            let target = self.sweep_scratch[index];
            let extra = Some(combat::SWEEP_KNOCKBACK);
            self.melee_damage(hit, target, hit.strength, extra, now, notices);
        }
        let [dx, dz] = [-hit.facing[0], -hit.facing[1]];
        self.effects.particles(
            Particles {
                particle: Particle::SWEEP_ATTACK,
                pos: [origin.x + dx, origin.y + hit.height * 0.5, origin.z + dz],
                offset: [dx as f32, 0.0, dz as f32],
                speed: 0.0,
                count: 0,
            },
            None,
        );
    }

    /// Damage one player through its rules and armor, then knock it back:
    /// away from the attacker on a fresh hit, plus `extra` strength along the
    /// attacker's facing. Returns the health lost, or None when ignored.
    fn melee_damage(
        &mut self,
        hit: Hit,
        target: i32,
        amount: f32,
        extra: Option<f64>,
        now: Instant,
        notices: &mut Vec<Rc<Input>>,
    ) -> Option<f32> {
        let damage = Damage { kind: DamageKind::PlayerAttack, amount, source: Some(hit.attacker) };
        let victim = self.players.get_mut(&target)?;
        let before = victim.state.life.health();
        let fresh = !victim.state.life.damage_window(now);
        let mut update = PlayerUpdate::default();
        let result = if hit.pvp && victim.state.life.rules.pvp {
            victim.state.attacked(damage, now, &mut update)
        } else {
            DamageResult::Ignored
        };
        let DamageResult::Hurt { died, .. } = result else {
            return None;
        };
        if victim.active {
            self.effects.hurt(target, damage);
        }
        let transform = victim.state.movement.transform();
        let resistance = Armor::worn(&victim.state.equipment).knockback_resistance;
        let rng = &mut self.rng;
        let mut velocity = None;
        if fresh {
            let away = [hit.origin.x - transform.x, hit.origin.z - transform.z];
            velocity = combat::knockback(
                victim.state.knockback_base(),
                transform.on_ground,
                combat::HURT_KNOCKBACK,
                resistance,
                away,
                || rng.unit(),
            );
        }
        if let Some(strength) = extra {
            let base = velocity.unwrap_or_else(|| victim.state.knockback_base());
            velocity = combat::knockback(
                base,
                transform.on_ground,
                strength,
                resistance,
                hit.facing,
                || rng.unit(),
            )
            .or(velocity);
        }
        if let Some(velocity) = velocity {
            victim.state.push(velocity, &mut update);
        }
        let dealt = before - victim.state.life.health();
        let input = victim.input.clone();
        if died {
            self.events.push(GameEvent::Died { player: target, killer: Some(hit.attacker) });
        }
        if died {
            // Only a kill names its attacker.
            update.killer = self
                .players
                .get(&hit.attacker)
                .map(|killer| Rc::from(killer.snapshot.profile.username.as_str()));
        }
        if update.changed() {
            input.update.borrow_mut().merge(update);
            notices.push(input);
        }
        Some(dealt)
    }

    fn attack_sound(&mut self, sound: Sound, pos: [f64; 3]) {
        self.effects.sound(sound, pos, 1.0, 1.0, self.rng.next() as i64, None);
    }

    /// Spectator cameras read other players' authority after every player advanced.
    fn resolve_cameras(&mut self, notices: &mut Vec<Rc<Input>>) {
        use crate::player::{CameraRequest, CameraTarget};
        while let Some(id) = self.camera_scratch.pop() {
            let Some(player) = self.players.get(&id) else { continue };
            let observer = player.state.movement.transform();
            // A new follow targets an entity the client attacked, within its reach.
            let reach = matches!(player.state.camera_request(),
                Some(CameraRequest::Follow(target)) if Some(target) != player.state.camera());
            let target = player
                .state
                .camera_target()
                .filter(|target| *target != id)
                .and_then(|target| self.players.get(&target))
                .filter(|target| {
                    target.active && !target.input.closed.get() && target.state.life.alive()
                })
                .map(|target| CameraTarget {
                    transform: target.state.movement.transform(),
                    attached: target.state.camera().is_some(),
                })
                .filter(|target| {
                    let [dx, dy, dz] = [
                        target.transform.x - observer.x,
                        target.transform.y - observer.y,
                        target.transform.z - observer.z,
                    ];
                    !reach || dx * dx + dy * dy + dz * dz <= SPECTATE_REACH * SPECTATE_REACH
                });
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            let input = &player.input;
            let mut update = PlayerUpdate::default();
            player.state.resolve_camera(target, &mut input.proposals.borrow_mut(), &mut update);
            let accepted = player.state.movement.transform();
            let old = input.accepted.replace(accepted);
            if old != accepted {
                input.transform.set(Some(accepted));
            }
            input.ready.set(
                player.state.life.alive()
                    && player.state.life.pending_teleport().is_none()
                    && (player.active || input.activate.get()),
            );
            if update.changed() || old.chunk() != accepted.chunk() {
                input.update.borrow_mut().merge(update);
                notices.push(input.clone());
            }
        }
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
            let camera_attached = player.state.camera().is_some();
            if player.snapshot.game_mode != player.state.game_mode.current
                || player.snapshot.camera_attached != camera_attached
            {
                if player.snapshot.game_mode != player.state.game_mode.current {
                    self.roster_revision = self.roster_revision.wrapping_add(1);
                }
                player.snapshot.game_mode = player.state.game_mode.current;
                player.snapshot.camera_attached = camera_attached;
                // Spectator visibility depends on both modes.
                self.spatial_revision = self.spatial_revision.wrapping_add(1);
                player_changed = true;
            }
            let sneaking = player.state.movement.sneaking();
            let pose = player.state.movement.pose();
            let swimming = player.state.movement.swimming();
            let sprinting = player.state.movement.sprinting();
            let using = player.state.using();
            let health = player.state.life.health();
            let visible = player.state.life.visible(now);
            let generation = player.state.life.generation;
            if player.snapshot.sneaking != sneaking
                || player.snapshot.pose != pose
                || player.snapshot.swimming != swimming
                || player.snapshot.sprinting != sprinting
                || player.snapshot.using != using
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
                player.snapshot.using = using;
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
