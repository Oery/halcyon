//! The owner-side facade a game uses to read and direct its instance's players.

use std::rc::Rc;

use super::session::Input;
use super::{InstanceId, Registry, Transform};
use crate::game::Destination;
use crate::lifecycle::PlayerRules;
use crate::player::{PlayerAction, PlayerUpdate};
use crate::scoreboard::{
    DisplayOverride, DisplaySlot, ObjectiveId, ObjectiveSpec, Score, ScoreHolder, ScoreboardError,
    Text,
};

/// Chat lines a single update holds; older lines are dropped first.
pub const MAX_MESSAGES: usize = 16;

pub struct GameCtx<'r> {
    pub(super) id: InstanceId,
    pub(super) registry: &'r mut Registry,
    pub(super) world: &'r halcyon_world::World,
    pub(super) notices: &'r mut Vec<Rc<Input>>,
    pub(super) close: &'r mut bool,
}

impl GameCtx<'_> {
    /// Reliable displays, published through the connection's bounded writer.
    pub fn scoreboards(&mut self) -> Scoreboards<'_> {
        self.registry.scoreboards.bind(self.id);
        Scoreboards { registry: self.registry }
    }
    // Game APIs are intentionally available before a concrete game consumes them.
    #[allow(dead_code)]
    pub fn instance(&self) -> InstanceId {
        self.id
    }

    pub fn map_spawn(&self) -> Transform {
        let (x, y, z, yaw, pitch) = self.world.spawn;
        Transform { x, y, z, yaw, pitch, on_ground: false }
    }

    /// Admitted players in entity order, including those still loading.
    /// Allocation-free; collect it first to act on players while iterating.
    pub fn players(&self) -> impl Iterator<Item = i32> + '_ {
        self.registry.members.iter().copied().filter(|id| self.connected(*id))
    }

    pub fn player_count(&self) -> usize {
        self.players().count()
    }

    /// A departed player stays a member until the owner's next cleanup.
    fn connected(&self, player: i32) -> bool {
        self.registry.players.get(&player).is_some_and(|entry| !entry.input.closed.get())
    }

    pub fn name(&self, player: i32) -> Option<&str> {
        Some(&self.registry.players.get(&player)?.snapshot.profile.username)
    }

    pub fn position(&self, player: i32) -> Option<Transform> {
        Some(self.registry.players.get(&player)?.state.movement.transform())
    }

    #[allow(dead_code)]
    pub fn alive(&self, player: i32) -> bool {
        self.registry.players.get(&player).is_some_and(|player| player.state.life.alive())
    }

    /// Moves a player to the nearest clear spot at `transform`. Before the
    /// player is visible this sets its spawn; afterwards it is a teleport.
    pub fn teleport(&mut self, player: i32, transform: Transform) -> bool {
        let view = self.registry.arena.view(self.world);
        let Some(transform) = crate::movement::resolve_spawn(&view, transform) else {
            return false;
        };
        let Some(entry) = self.registry.players.get_mut(&player) else {
            return false;
        };
        if entry.active || entry.state.life.pending_teleport().is_some() {
            return entry
                .input
                .actions
                .borrow_mut()
                .push(entry.state.life.generation, PlayerAction::Teleport(transform));
        }
        entry.state.movement.place(transform);
        entry.snapshot.transform = transform;
        entry.input.accepted.set(transform);
        true
    }

    /// Rules apply immediately; the respawn point must be clear terrain.
    pub fn set_rules(&mut self, player: i32, mut rules: PlayerRules) -> bool {
        let view = self.registry.arena.view(self.world);
        let Some(respawn) = crate::movement::resolve_spawn(&view, rules.respawn) else {
            return false;
        };
        rules.respawn = respawn;
        let Some(entry) = self.registry.players.get_mut(&player) else {
            return false;
        };
        entry.state.life.rules = rules;
        true
    }

    /// Full health, a fresh life and the rules' respawn point.
    pub fn reset(&mut self, player: i32) -> bool {
        self.registry.players.get(&player).is_some_and(|entry| {
            entry.input.actions.borrow_mut().push(entry.state.life.generation, PlayerAction::Reset)
        })
    }

    #[allow(dead_code)]
    pub fn message(&mut self, player: i32, text: &str) {
        let text: Rc<str> = Rc::from(text);
        self.update(player, PlayerUpdate { messages: vec![text], ..Default::default() });
    }

    pub fn broadcast(&mut self, text: &str) {
        let text: Rc<str> = Rc::from(text);
        // Index the members: `update` needs the context mutably.
        for index in 0..self.registry.members.len() {
            let player = self.registry.members[index];
            if self.connected(player) {
                let update = PlayerUpdate { messages: vec![text.clone()], ..Default::default() };
                self.update(player, update);
            }
        }
    }

    /// The player's connection leaves this instance for `destination`.
    pub fn send_to(&mut self, player: i32, destination: Destination) {
        self.update(player, PlayerUpdate { destination: Some(destination), ..Default::default() });
    }

    /// The instance shuts down once its last player has left.
    pub fn close(&mut self) {
        *self.close = true;
    }

    fn update(&mut self, player: i32, update: PlayerUpdate) {
        if let Some(entry) = self.registry.players.get(&player) {
            entry.input.update.borrow_mut().merge(update);
            self.notices.push(entry.input.clone());
        }
    }
}

/// Owner-side scoreboard operations. IDs belong to this instance and never
/// become valid again after removal. New viewers inherit the shared slots.
pub struct Scoreboards<'r> {
    registry: &'r mut Registry,
}

// These APIs intentionally precede consumers in future minigames.
#[allow(dead_code)]
impl Scoreboards<'_> {
    fn player(&self, player: i32) -> Result<&str, ScoreboardError> {
        let entry = self.registry.players.get(&player).ok_or(ScoreboardError::Player)?;
        if entry.input.closed.get() {
            return Err(ScoreboardError::Player);
        }
        Ok(&entry.snapshot.profile.username)
    }

    pub fn create_objective(
        &mut self,
        spec: ObjectiveSpec,
    ) -> Result<ObjectiveId, ScoreboardError> {
        self.registry.scoreboards.create(None, spec)
    }

    /// Only this viewer may display the objective; it is removed on departure.
    pub fn create_personal_objective(
        &mut self,
        viewer: i32,
        spec: ObjectiveSpec,
    ) -> Result<ObjectiveId, ScoreboardError> {
        self.player(viewer)?;
        self.registry.scoreboards.create(Some(viewer), spec)
    }

    pub fn update_objective(
        &mut self,
        id: ObjectiveId,
        spec: ObjectiveSpec,
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.update(id, spec)
    }

    pub fn remove_objective(&mut self, id: ObjectiveId) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.remove(id)
    }

    /// None hides the shared slot. Personal overrides continue to apply.
    pub fn display(
        &mut self,
        slot: DisplaySlot,
        objective: Option<ObjectiveId>,
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.display(slot, objective)
    }

    pub fn display_for(
        &mut self,
        viewer: i32,
        slot: DisplaySlot,
        value: DisplayOverride,
    ) -> Result<(), ScoreboardError> {
        self.player(viewer)?;
        self.registry.scoreboards.display_for(viewer, slot, value)
    }

    pub fn set_score(
        &mut self,
        objective: ObjectiveId,
        holder: ScoreHolder,
        score: Score,
    ) -> Result<(), ScoreboardError> {
        let name: Rc<str> = match &holder {
            ScoreHolder::Player(player) => Rc::from(self.player(*player)?),
            ScoreHolder::Entry(key) => {
                if key.is_empty() || key.len() > 64 {
                    return Err(ScoreboardError::InvalidKey);
                }
                Rc::from(format!("$halcyon:{key}"))
            }
        };
        self.registry.scoreboards.set_score(objective, holder, name, score)
    }

    pub fn reset_score(
        &mut self,
        objective: ObjectiveId,
        holder: &ScoreHolder,
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.reset_score(objective, holder)
    }

    /// Replaces all scores in this objective with an ordered text sidebar.
    /// It does not change which viewers display the objective.
    pub fn sidebar(
        &mut self,
        objective: ObjectiveId,
        title: Text,
        lines: &[Text],
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.sidebar(objective, title, lines)
    }

    /// Updates one existing line (zero-based), without rebuilding other lines.
    pub fn set_sidebar_line(
        &mut self,
        objective: ObjectiveId,
        line: usize,
        text: Text,
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.set_sidebar_line(objective, line, text)
    }
}
