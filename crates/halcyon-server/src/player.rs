//! Authoritative per-player gameplay. No packet encoding or socket I/O lives here.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use halcyon_world::BlockView;
#[cfg(test)]
use halcyon_world::World;
use mc_protocol::packets::client::InteractionHand;
use mc_protocol::types::{GameMode, ItemStack};

use crate::instance::Transform;
use crate::inventory::{
    HotbarSlot, Inventory, InventoryAction, InventoryChange, InventoryError, InventorySlot,
    InventorySnapshot,
};
use crate::lifecycle::{Damage, DamageKind, DamageResult, PlayerLife, PlayerRules, Teleport};
use crate::movement::{Advance, MovementInput, MovementState, ProposalQueue};
use crate::text::{Text, Title};

const ACTIONS_PER_STEP: usize = 4;
const MAX_ACTIONS: usize = 32;
pub const MAX_USED_BLOCKS: usize = 4096;

/// Arena hooks deliberately precede the first minigame implementation.
#[allow(dead_code)]
pub enum PlayerAction {
    SetFlying(bool),
    SetGameMode {
        mode: GameMode,
        reply: glommio::channels::oneshot::Sender<Result<(), PlayerActionError>>,
    },
    Teleport(Transform),
    TeleportRequest {
        destination: Transform,
        reply: glommio::channels::oneshot::Sender<Result<(), PlayerActionError>>,
    },
    Damage(Damage),
    Heal(f32),
    Rules(PlayerRules),
    Respawn,
    Reset,
    Inventory(InventoryAction),
    /// Trusted owner input; None returns to client-predicted movement.
    Drive(Option<MovementInput>),
    /// Item use in progress; it ends when the hand's item changes or the life ends.
    UseItem(Option<InteractionHand>),
    /// Spectator camera and spectator-menu teleports, resolved against other players.
    Spectate(CameraRequest),
    /// Melee attack on another entity, resolved against other players.
    Attack(i32),
    EditInventory {
        edit: InventoryEdit,
        reply: glommio::channels::oneshot::Sender<Result<ItemStack, InventoryEditError>>,
    },
    GiveCreative {
        stack: ItemStack,
        reply: glommio::channels::oneshot::Sender<Result<ItemStack, PlayerActionError>>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameModeState {
    pub current: GameMode,
    pub previous: Option<GameMode>,
    pub flying: bool,
}

impl GameModeState {
    pub fn ability_flags(self) -> u8 {
        match self.current {
            // Invulnerable, allow flight, instant break; flight itself is opt-in.
            GameMode::Creative => 0x0d | if self.flying { 0x02 } else { 0 },
            // Invulnerable, always flying, allow flight.
            GameMode::Spectator => 0x07,
            _ => 0,
        }
    }

    /// Creative and Spectator players ignore everything except void rules.
    pub fn invulnerable(self) -> bool {
        matches!(self.current, GameMode::Creative | GameMode::Spectator)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraRequest {
    /// View through another player until sneaking, a mode change or its loss.
    Follow(i32),
    Stop,
    TeleportTo(i32),
}

/// A melee attack awaiting resolution, with the attack strength it was made at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingAttack {
    pub target: i32,
    /// Ticks since the attack strength was last reset.
    pub ticks: u32,
}

/// Another player's current authority, read by the owner for camera resolution.
#[derive(Clone, Copy, Debug)]
pub struct CameraTarget {
    pub transform: Transform,
    /// A player viewing through another camera cannot itself be followed.
    pub attached: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlayerActionError {
    #[error("Player not found.")]
    NotFound,
    #[error("More than one player matches that name.")]
    Ambiguous,
    #[error("The player is not ready for this action.")]
    NotReady,
    #[error("The player's action queue is full. Try again shortly.")]
    QueueFull,
    #[error("The player disconnected before the action completed.")]
    Closed,
    #[error("The player respawned before the action completed. Try again.")]
    StaleLife,
    #[error("The destination is outside loaded terrain, obstructed or unsupported.")]
    InvalidDestination,
    #[error("This command requires Creative mode.")]
    CreativeRequired,
}

#[cfg(test)]
#[path = "tests/tests_player.rs"]
mod tests;

/// Deliberate game-owned changes; replacing a loadout intentionally replaces its cursor too.
#[allow(dead_code)]
pub enum InventoryEdit {
    SetSlot { slot: InventorySlot, stack: ItemStack },
    Loadout { slots: Box<[ItemStack; InventorySlot::COUNT]>, selected: HotbarSlot },
    Clear,
    Give(ItemStack),
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryEditError {
    #[error("player action queue is full")]
    QueueFull,
    #[error("player owner is closed")]
    Closed,
    #[error("the player life changed before the inventory edit")]
    StaleLife,
    #[error(transparent)]
    Invalid(#[from] InventoryError),
}

struct QueuedAction {
    order: u64,
    generation: u64,
    action: PlayerAction,
}

pub struct ActionQueue {
    pending: VecDeque<QueuedAction>,
    overflow: bool,
    submitted: u64,
    completed: u64,
    pub barrier: Option<u64>,
}

impl Default for ActionQueue {
    fn default() -> Self {
        Self {
            pending: VecDeque::with_capacity(MAX_ACTIONS),
            overflow: false,
            submitted: 0,
            completed: 0,
            barrier: None,
        }
    }
}

impl ActionQueue {
    pub fn submitted(&self) -> u64 {
        self.submitted
    }
    pub fn completed(&self) -> u64 {
        self.pending.front().map_or(self.submitted, |q| q.order - 1).max(self.completed)
    }
    #[cfg(test)]
    pub(crate) fn capacity_payload_bytes(&self) -> usize {
        self.pending.capacity() * std::mem::size_of::<QueuedAction>()
    }
    pub fn push(&mut self, generation: u64, action: PlayerAction) -> bool {
        if self.pending.len() == MAX_ACTIONS {
            self.overflow = true;
            return false;
        }
        self.submitted += 1;
        self.pending.push_back(QueuedAction { order: self.submitted, generation, action });
        true
    }
}

#[derive(Clone, Debug)]
pub enum TitleUpdate {
    Show(Rc<Title>),
    Clear,
}

/// Replaceable self state; it never accumulates one packet per simulation step.
#[derive(Default)]
pub struct PlayerUpdate {
    pub game_mode: Option<GameModeState>,
    /// The followed entity, or None to return the camera to the player.
    pub camera: Option<Option<i32>>,
    pub block_resync: Option<Transform>,
    /// Cells changed by accepted block uses, resent before `block_ack`.
    pub used_blocks: Vec<crate::arena::BlockPos>,
    /// Highest processed block-use sequence; it survives a respawn.
    pub block_ack: Option<i32>,
    pub teleport: Option<Teleport>,
    pub health: Option<f32>,
    pub damage: Option<Damage>,
    pub death: Option<Damage>,
    /// The attacking player's name when `death` was a melee kill.
    pub killer: Option<Rc<str>>,
    /// Knockback for the player's own client, in blocks per tick.
    pub velocity: Option<[f64; 3]>,
    pub respawn: bool,
    pub inventory: Option<InventorySnapshot>,
    pub inventory_slots: u64,
    pub inventory_full: bool,
    /// Game chat lines, oldest first and bounded.
    pub messages: Vec<Rc<Text>>,
    /// The latest action bar line replaces any earlier one.
    pub action_bar: Option<Rc<Text>>,
    /// The latest title change replaces any earlier one.
    pub title: Option<TitleUpdate>,
    /// The game sends this connection to another instance.
    pub destination: Option<crate::game::Destination>,
}

impl PlayerUpdate {
    fn clear_life_events(&mut self) {
        self.block_resync = None;
        self.damage = None;
        self.death = None;
        self.killer = None;
        self.velocity = None;
        self.teleport = None;
    }

    pub fn merge(&mut self, newer: Self) {
        if newer.game_mode.is_some() {
            self.game_mode = newer.game_mode;
        }
        if newer.camera.is_some() {
            self.camera = newer.camera;
        }
        if newer.respawn {
            // A replacement life supersedes transient events from the old one.
            self.clear_life_events();
        }
        if newer.teleport.is_some() {
            self.teleport = newer.teleport;
        }
        if newer.block_resync.is_some() {
            self.block_resync = newer.block_resync;
        }
        for pos in newer.used_blocks {
            if !self.used_blocks.contains(&pos) {
                self.used_blocks.push(pos);
            }
        }
        self.block_ack = self.block_ack.max(newer.block_ack);
        if newer.health.is_some() {
            self.health = newer.health;
        }
        if newer.damage.is_some() {
            self.damage = newer.damage;
        }
        if newer.death.is_some() {
            self.death = newer.death;
            self.killer = newer.killer;
        }
        if newer.velocity.is_some() {
            self.velocity = newer.velocity;
        }
        self.respawn |= newer.respawn;
        if newer.inventory.is_some() {
            self.inventory = newer.inventory;
        }
        self.inventory_slots |= newer.inventory_slots;
        self.inventory_full |= newer.inventory_full;
        self.messages.extend(newer.messages);
        let excess = self.messages.len().saturating_sub(crate::instance::MAX_MESSAGES);
        self.messages.drain(..excess);
        if newer.action_bar.is_some() {
            self.action_bar = newer.action_bar;
        }
        if newer.title.is_some() {
            self.title = newer.title;
        }
        if newer.destination.is_some() {
            self.destination = newer.destination;
        }
    }

    pub fn changed(&self) -> bool {
        !self.used_blocks.is_empty()
            || self.game_mode.is_some()
            || self.camera.is_some()
            || self.block_resync.is_some()
            || self.block_ack.is_some()
            || self.teleport.is_some()
            || self.health.is_some()
            || self.damage.is_some()
            || self.death.is_some()
            || self.velocity.is_some()
            || self.respawn
            || self.inventory.is_some()
            || !self.messages.is_empty()
            || self.action_bar.is_some()
            || self.title.is_some()
            || self.destination.is_some()
    }
}

pub struct PlayerState {
    pub mining: Option<crate::interaction::Mining>,
    pub game_mode: GameModeState,
    pub movement: MovementState,
    pub life: PlayerLife,
    pub inventory: Inventory,
    pub equipment: Rc<[ItemStack; 6]>,
    /// The used hand and the selection when its use began.
    using: Option<(InteractionHand, HotbarSlot)>,
    movement_input: Option<MovementInput>,
    resync_after: Option<Instant>,
    camera: Option<i32>,
    camera_request: Option<CameraRequest>,
    /// Vanilla's attackStrengthTicker, in simulation steps. Assumes 20 Hz
    /// steps; a faster simulation rate must scale it to real time.
    attack_ticks: u32,
    attack: Option<PendingAttack>,
}

impl PlayerState {
    pub fn new(transform: Transform) -> Self {
        Self {
            mining: None,
            game_mode: GameModeState::default(),
            movement: MovementState::new(transform),
            life: PlayerLife::new(transform),
            inventory: Inventory::default(),
            equipment: Rc::new(std::array::from_fn(|_| ItemStack::default())),
            using: None,
            movement_input: None,
            resync_after: None,
            camera: None,
            camera_request: None,
            attack_ticks: 0,
            attack: None,
        }
    }

    pub fn using(&self) -> Option<InteractionHand> {
        self.using.map(|(hand, _)| hand)
    }

    pub fn spectator(&self) -> bool {
        self.game_mode.current == GameMode::Spectator
    }

    /// The followed player, while this spectator views through its camera.
    pub fn camera(&self) -> Option<i32> {
        self.camera
    }

    /// The player whose authority the next `resolve_camera` needs, if any.
    pub fn camera_target(&self) -> Option<i32> {
        match self.camera_request {
            Some(CameraRequest::Follow(id) | CameraRequest::TeleportTo(id)) => Some(id),
            Some(CameraRequest::Stop) => None,
            None => self.camera,
        }
    }

    pub fn camera_request(&self) -> Option<CameraRequest> {
        self.camera_request
    }

    pub fn attack_pending(&self) -> bool {
        self.attack.is_some()
    }

    pub fn take_attack(&mut self) -> Option<PendingAttack> {
        self.attack.take()
    }

    /// Attacks, swings and switching to a different item restart the charge.
    pub fn reset_attack_strength(&mut self) {
        self.attack_ticks = 0;
    }

    /// Owner-driven bodies keep real velocity; human clients own theirs.
    pub fn driven(&self) -> bool {
        self.movement_input.is_some()
    }

    /// The motion knockback starts from. The server does not track a human
    /// client's velocity, which vanilla also leaves at rest between pushes.
    pub fn knockback_base(&self) -> [f64; 3] {
        if self.driven() {
            self.movement.motion.velocity
        } else {
            [0.0; 3]
        }
    }

    /// Apply knockback to a driven body, or send it to the player's client.
    pub fn push(&mut self, velocity: [f64; 3], update: &mut PlayerUpdate) {
        if self.driven() {
            self.movement.motion.velocity = velocity;
        } else {
            update.velocity = Some(velocity);
        }
    }

    /// Damage dealt by another player's attack, after its rules allowed it.
    pub fn attacked(
        &mut self,
        damage: Damage,
        now: Instant,
        update: &mut PlayerUpdate,
    ) -> DamageResult {
        let result = self.apply_damage(damage, now);
        self.damaged(damage, result, update);
        result
    }

    pub fn camera_pending(&self) -> bool {
        self.camera.is_some() || self.camera_request.is_some()
    }

    /// Return the view to this player at its current authoritative position,
    /// which the client otherwise does not know while viewing elsewhere.
    fn detach_camera(&mut self, proposals: &mut ProposalQueue, update: &mut PlayerUpdate) {
        if self.camera.take().is_some() {
            update.camera = Some(None);
            self.correct_movement(proposals, update);
        }
    }

    /// Runs after every player advanced, so `target` is that player's current
    /// authority. Following snaps silently; only attach and detach teleport.
    pub fn resolve_camera(
        &mut self,
        target: Option<CameraTarget>,
        proposals: &mut ProposalQueue,
        update: &mut PlayerUpdate,
    ) {
        let request = self.camera_request.take();
        if !self.spectator() || !self.life.alive() {
            self.detach_camera(proposals, update);
            return;
        }
        match request {
            Some(CameraRequest::Stop) => self.detach_camera(proposals, update),
            Some(CameraRequest::TeleportTo(_)) => {
                if let Some(target) = target
                    && let Some(teleport) = self.life.teleport(target.transform)
                {
                    self.camera = None;
                    update.camera = Some(None);
                    self.movement.reset(target.transform);
                    proposals.clear();
                    update.teleport = Some(teleport);
                }
            }
            Some(CameraRequest::Follow(id)) => {
                if let Some(target) = target.filter(|target| !target.attached)
                    && self.camera != Some(id)
                    && let Some(teleport) = self.life.teleport(target.transform)
                {
                    self.camera = Some(id);
                    update.camera = Some(Some(id));
                    self.movement.reset(target.transform);
                    proposals.clear();
                    update.teleport = Some(teleport);
                }
            }
            None => match target.filter(|target| !target.attached) {
                Some(target) if self.camera.is_some() => {
                    self.movement.reset(target.transform);
                    proposals.clear();
                }
                _ => self.detach_camera(proposals, update),
            },
        }
    }

    fn teleport(
        &mut self,
        world: &impl BlockView,
        transform: Transform,
        proposals: &mut ProposalQueue,
        update: &mut PlayerUpdate,
    ) -> bool {
        self.movement.set_powder_snow_walk(self.inventory.can_walk_on_powder_snow());
        if self.movement.supports_destination(world, transform)
            && let Some(teleport) = self.life.teleport(transform)
        {
            self.mining = None;
            if self.camera.take().is_some() {
                update.camera = Some(None);
            }
            self.movement.reset(transform);
            self.movement_input = None;
            proposals.clear();
            update.teleport = Some(teleport);
            true
        } else {
            false
        }
    }

    /// Synchronize to already-resolved authority, preserving its valid compact
    /// pose and fall baseline. A standing-only teleport check could reject a
    /// correction inside a low tunnel and silently leave the client desynced.
    fn correct_movement(&mut self, proposals: &mut ProposalQueue, update: &mut PlayerUpdate) {
        self.mining = None;
        if let Some(teleport) = self.life.teleport(self.movement.transform()) {
            self.movement.motion = Default::default();
            self.movement_input = None;
            proposals.clear();
            update.teleport = Some(teleport);
        }
    }

    fn respawn(
        &mut self,
        world: &impl BlockView,
        reset: bool,
        proposals: &mut ProposalQueue,
        update: &mut PlayerUpdate,
    ) {
        if !reset && self.life.alive() {
            return;
        }
        let Some(destination) = crate::movement::resolve_spawn(world, self.life.rules.respawn)
        else {
            return;
        };
        if reset || self.life.respawn() {
            if reset {
                self.life.reset();
            }
            update.clear_life_events();
            self.using = None;
            self.attack = None;
            self.attack_ticks = 0;
            self.inventory.begin_life();
            update.health = Some(self.life.health());
            update.respawn = true;
            self.game_mode.flying = self.spectator();
            if self.camera.take().is_some() {
                update.camera = Some(None);
            }
            self.camera_request = None;
            update.game_mode = Some(self.game_mode);
            update.inventory = Some(self.inventory.snapshot());
            update.inventory_full = true;
            self.teleport(world, destination, proposals, update);
        }
    }

    fn damaged(&mut self, damage: Damage, result: DamageResult, update: &mut PlayerUpdate) {
        if let DamageResult::Hurt { health, died } = result {
            update.health = Some(health);
            update.damage = Some(damage);
            if died {
                self.mining = None;
                update.death = Some(damage);
                self.using = None;
                self.inventory.cancel_drag();
                self.movement_input = None;
                self.movement.reset(self.movement.transform());
            }
        }
    }

    fn apply_damage(&mut self, damage: Damage, now: Instant) -> DamageResult {
        if self.game_mode.invulnerable() && damage.kind != DamageKind::Void {
            DamageResult::Ignored
        } else if damage.kind == DamageKind::PlayerAttack {
            // Fall, void and suffocation bypass armor in vanilla.
            let armor = crate::combat::Armor::worn(&self.equipment);
            self.life.damage_absorbed(damage, now, |amount| armor.absorb(amount))
        } else {
            self.life.damage(damage, now)
        }
    }

    pub(crate) fn inventory_changed(
        &mut self,
        changed: InventoryChange,
        update: &mut PlayerUpdate,
    ) {
        if self.mining.as_ref().is_some_and(|m| !m.hand_valid(&self.inventory)) {
            self.mining = None;
        }
        let previous = changed
            .equipment_changed
            .then(|| std::mem::replace(&mut self.equipment, Rc::new(self.inventory.equipment())));
        // Vanilla stops a use when the hand's item or the held selection changes.
        if let Some((hand, selected)) = self.using
            && (previous
                .as_ref()
                .is_some_and(|old| old[hand as usize] != self.equipment[hand as usize])
                || (hand == InteractionHand::Main && selected != self.inventory.selected()))
        {
            self.using = None;
        }
        let kind = |stack: &ItemStack| if stack.is_empty() { 0 } else { stack.item_id() };
        if previous.is_some_and(|old| kind(&old[0]) != kind(&self.equipment[0])) {
            self.reset_attack_strength();
        }
        if changed.has_changes() {
            update.inventory_slots |= changed.changed_slots;
            update.inventory_full |= changed.full_sync;
            update.inventory = Some(self.inventory.snapshot());
        }
    }

    fn edit_inventory(
        &mut self,
        edit: InventoryEdit,
        update: &mut PlayerUpdate,
    ) -> Result<ItemStack, InventoryEditError> {
        let (remainder, changed) = match edit {
            InventoryEdit::SetSlot { slot, stack } => {
                (ItemStack::default(), self.inventory.set_slot(slot, stack)?)
            }
            InventoryEdit::Loadout { slots, selected } => {
                (ItemStack::default(), self.inventory.replace_loadout(*slots, selected)?)
            }
            InventoryEdit::Clear => (ItemStack::default(), self.inventory.reset()),
            InventoryEdit::Give(stack) => self.inventory.give(stack),
        };
        self.inventory_changed(changed, update);
        Ok(remainder)
    }

    pub fn advance(
        &mut self,
        world: &impl BlockView,
        proposals: &mut ProposalQueue,
        actions: &mut ActionQueue,
        now: Instant,
        respawn_requested: bool,
    ) -> (PlayerUpdate, Option<Advance>) {
        let mut update = PlayerUpdate::default();
        if respawn_requested {
            self.respawn(world, false, proposals, &mut update);
        }
        if std::mem::take(&mut actions.overflow) {
            update.game_mode = Some(self.game_mode);
            update.inventory = Some(self.inventory.snapshot());
            update.inventory_full = true;
        }
        for _ in 0..ACTIONS_PER_STEP {
            if actions
                .pending
                .front()
                .is_some_and(|queued| actions.barrier.is_some_and(|barrier| queued.order > barrier))
            {
                break;
            }
            let Some(queued) = actions.pending.pop_front() else {
                break;
            };
            actions.completed = queued.order;
            if matches!(queued.action, PlayerAction::Attack(_)) && self.attack.is_some() {
                // One attack resolves per step. Vanilla would land later ones
                // uncharged inside the damage window, so they are dropped
                // rather than queued behind other actions.
                continue;
            }
            if queued.generation != self.life.generation {
                match queued.action {
                    PlayerAction::EditInventory { reply, .. } => {
                        let _ = reply.send(Err(InventoryEditError::StaleLife));
                    }
                    PlayerAction::SetGameMode { reply, .. }
                    | PlayerAction::TeleportRequest { reply, .. } => {
                        let _ = reply.send(Err(PlayerActionError::StaleLife));
                    }
                    PlayerAction::GiveCreative { reply, .. } => {
                        let _ = reply.send(Err(PlayerActionError::StaleLife));
                    }
                    _ => {}
                }
                continue;
            }
            match queued.action {
                PlayerAction::GiveCreative { stack, reply } => {
                    let result = if self.game_mode.current != GameMode::Creative {
                        Err(PlayerActionError::CreativeRequired)
                    } else if !self.life.alive() || self.life.pending_teleport().is_some() {
                        Err(PlayerActionError::NotReady)
                    } else {
                        let (remainder, change) = self.inventory.give(stack);
                        self.inventory_changed(change, &mut update);
                        Ok(remainder)
                    };
                    let _ = reply.send(result);
                }
                PlayerAction::SetFlying(flying) => {
                    // Spectators cannot stop flying.
                    self.game_mode.flying = self.spectator()
                        || (flying
                            && self.game_mode.current == GameMode::Creative
                            && self.life.alive()
                            && self.life.pending_teleport().is_none());
                    update.game_mode = Some(self.game_mode);
                }
                PlayerAction::SetGameMode { mode, reply } => {
                    if !self.life.alive() || self.life.pending_teleport().is_some() {
                        let _ = reply.send(Err(PlayerActionError::NotReady));
                        continue;
                    }
                    if mode != self.game_mode.current {
                        self.mining = None;
                        self.game_mode.previous = Some(self.game_mode.current);
                        self.game_mode.current = mode;
                        self.game_mode.flying = self.spectator();
                        update.game_mode = Some(self.game_mode);
                        self.movement.set_spectator(self.spectator());
                        if self.spectator() {
                            // Spectators cannot use or hold an item use.
                            self.using = None;
                            self.movement_input = None;
                            self.inventory.cancel_drag();
                        } else {
                            self.camera_request = None;
                            self.detach_camera(proposals, &mut update);
                        }
                    }
                    let _ = reply.send(Ok(()));
                }
                PlayerAction::Teleport(transform) => {
                    self.teleport(world, transform, proposals, &mut update);
                }
                PlayerAction::TeleportRequest { destination, reply } => {
                    let result = if !self.life.alive() || self.life.pending_teleport().is_some() {
                        Err(PlayerActionError::NotReady)
                    } else if self.teleport(world, destination, proposals, &mut update) {
                        Ok(())
                    } else {
                        Err(PlayerActionError::InvalidDestination)
                    };
                    let _ = reply.send(result);
                }
                PlayerAction::Damage(damage) => {
                    let result = self.apply_damage(damage, now);
                    self.damaged(damage, result, &mut update);
                }
                PlayerAction::Heal(amount) => {
                    if self.life.heal(amount) {
                        update.health = Some(self.life.health());
                    }
                }
                PlayerAction::Rules(rules) => {
                    // Checked as a player body, even while this player is a spectator.
                    if crate::movement::resolve_spawn(world, rules.respawn) == Some(rules.respawn) {
                        self.life.rules = rules;
                    }
                }
                PlayerAction::Respawn | PlayerAction::Reset => {
                    let reset = matches!(queued.action, PlayerAction::Reset);
                    self.respawn(world, reset, proposals, &mut update);
                }
                PlayerAction::Inventory(action) => {
                    // Spectators may change the hotbar selection, but never rearrange items.
                    let permitted = !self.spectator()
                        || matches!(
                            action,
                            InventoryAction::Select { .. }
                                | InventoryAction::Close { .. }
                                | InventoryAction::RejectDrop
                                | InventoryAction::Resync
                        );
                    if self.life.alive() && permitted {
                        let changed = self.inventory.apply(action);
                        self.inventory_changed(changed, &mut update);
                    } else {
                        update.inventory_full = true;
                        update.inventory = Some(self.inventory.snapshot());
                    }
                }
                PlayerAction::Drive(input) => {
                    if self.life.alive()
                        && !self.spectator()
                        && self.life.pending_teleport().is_none()
                        && input
                            .is_none_or(|input| input.yaw.is_finite() && input.pitch.is_finite())
                    {
                        if self.movement_input.is_some() != input.is_some() {
                            self.movement.motion = Default::default();
                        }
                        self.movement_input = input;
                        proposals.clear();
                    }
                }
                PlayerAction::UseItem(hand) => {
                    if self.life.alive() && !self.spectator() {
                        self.using = hand.map(|hand| (hand, self.inventory.selected()));
                    }
                }
                PlayerAction::EditInventory { edit, reply } => {
                    let _ = reply.send(self.edit_inventory(edit, &mut update));
                }
                PlayerAction::Spectate(request) => {
                    if self.spectator() && self.life.alive() {
                        self.camera_request = Some(request);
                    }
                }
                PlayerAction::Attack(target) => {
                    if self.life.alive()
                        && !self.spectator()
                        && self.life.pending_teleport().is_none()
                    {
                        self.attack = Some(PendingAttack { target, ticks: self.attack_ticks });
                    }
                }
            }
        }
        if update.teleport.is_some() {
            // Queued movement belongs to the old baseline, even though an
            // ordinary teleport keeps the same player life generation.
            actions.pending.retain(|queued| !matches!(queued.action, PlayerAction::Drive(_)));
        }
        self.movement.set_flying(self.game_mode.flying);
        self.movement.set_spectator(self.spectator());
        self.attack_ticks = self.attack_ticks.saturating_add(1);
        if !self.life.alive() || self.life.pending_teleport().is_some() {
            if self.movement.suffocating(world) {
                let damage = Damage { kind: DamageKind::Suffocation, amount: 1.0, source: None };
                let result = self.apply_damage(damage, now);
                self.damaged(damage, result, &mut update);
            }
            proposals.clear();
            return (update, None);
        }
        self.movement.set_powder_snow_walk(self.inventory.can_walk_on_powder_snow());
        let movement = if let Some(input) = self.movement_input {
            proposals.clear();
            Some(self.movement.step(world, input))
        } else if proposals.is_empty() {
            None
        } else {
            Some(self.movement.advance(world, proposals))
        };
        if let Some(movement) = movement
            && movement.correction
        {
            if self.resync_after.is_none_or(|after| now >= after) {
                update.block_resync = Some(self.movement.transform());
                self.resync_after = Some(now + std::time::Duration::from_millis(250));
            }
            self.correct_movement(proposals, &mut update);
        }
        if self.movement.suffocating(world) {
            let damage = Damage { kind: DamageKind::Suffocation, amount: 1.0, source: None };
            let result = self.apply_damage(damage, now);
            self.damaged(damage, result, &mut update);
        }
        if !self.game_mode.invulnerable()
            && let Some(amount) = movement.and_then(|movement| movement.landed_fall_damage)
        {
            let result = self.life.landing_damage(amount, now);
            self.damaged(
                Damage { kind: DamageKind::Fall, amount, source: None },
                result,
                &mut update,
            );
        }
        let below_world = self.movement.transform().y < f64::from(halcyon_world::MIN_BLOCK_Y - 64);
        if below_world && self.life.rules.protected {
            self.teleport(world, self.life.rules.respawn, proposals, &mut update);
        } else {
            let result = self.life.void_damage(below_world, now);
            self.damaged(
                Damage { kind: DamageKind::Void, amount: 4.0, source: None },
                result,
                &mut update,
            );
        }
        (update, movement)
    }
}
