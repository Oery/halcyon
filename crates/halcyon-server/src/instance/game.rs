//! The owner-side facade a game uses to read and direct its instance's players.

use std::rc::Rc;

use super::session::Input;
use super::{InstanceId, Registry, Transform};
use crate::arena::{BlockPos, EditError};
use crate::game::Destination;
use crate::inventory::{HotbarSlot, InventorySlot};
use crate::lifecycle::PlayerRules;
use crate::player::{InventoryEdit, PlayerAction, PlayerUpdate, TitleUpdate};
use crate::scoreboard::{
    DisplayOverride, DisplaySlot, LineId, ObjectiveId, ObjectiveSpec, Score, ScoreHolder,
    ScoreboardError, SidebarLayout,
};
use crate::text::{Text, Title};
use mc_protocol::types::ItemStack;

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

    /// Queued healing, up to full health.
    pub fn heal(&mut self, player: i32, amount: f32) -> bool {
        self.registry.players.get(&player).is_some_and(|entry| {
            entry
                .input
                .actions
                .borrow_mut()
                .push(entry.state.life.generation, PlayerAction::Heal(amount))
        })
    }

    /// Full health, a fresh life and the rules' respawn point.
    pub fn reset(&mut self, player: i32) -> bool {
        self.registry.players.get(&player).is_some_and(|entry| {
            entry.input.actions.borrow_mut().push(entry.state.life.generation, PlayerAction::Reset)
        })
    }

    /// Replaces the player's inventory. Queued like `reset`; queue it before
    /// a `reset` in the same step, since a fresh life drops older actions.
    pub fn set_loadout(
        &mut self,
        player: i32,
        slots: [ItemStack; InventorySlot::COUNT],
        selected: HotbarSlot,
    ) -> bool {
        let Some(entry) = self.registry.players.get(&player) else {
            return false;
        };
        // Nobody waits for the result; a failed edit leaves the inventory as it was.
        let (reply, _) = glommio::channels::oneshot::oneshot();
        let edit = InventoryEdit::Loadout { slots: Box::new(slots), selected };
        entry
            .input
            .actions
            .borrow_mut()
            .push(entry.state.life.generation, PlayerAction::EditInventory { edit, reply })
    }

    /// Trusted terrain edits through the instance's overlay, in bounded
    /// transactions. Stops at the first rejected batch.
    pub fn set_blocks(
        &mut self,
        cells: &[(BlockPos, halcyon_world::BlockStateId)],
    ) -> Result<(), EditError> {
        for batch in cells.chunks(halcyon_world::blocks::MAX_EDIT_CELLS) {
            self.registry.arena.edit_all(self.world, batch)?;
        }
        Ok(())
    }

    /// A system chat line for one player.
    #[allow(dead_code)]
    pub fn message(&mut self, player: i32, text: impl Into<Text>) {
        let Some(text) = checked(text.into()) else { return };
        self.update(player, PlayerUpdate { messages: vec![text], ..Default::default() });
    }

    /// A system chat line for every connected player.
    pub fn broadcast(&mut self, text: impl Into<Text>) {
        let Some(text) = checked(text.into()) else { return };
        self.update_connected(|| PlayerUpdate {
            messages: vec![text.clone()],
            ..Default::default()
        });
    }

    /// Replaces the line above the player's hotbar.
    #[allow(dead_code)]
    pub fn action_bar(&mut self, player: i32, text: impl Into<Text>) {
        let Some(text) = checked(text.into()) else { return };
        self.update(player, PlayerUpdate { action_bar: Some(text), ..Default::default() });
    }

    #[allow(dead_code)]
    pub fn broadcast_action_bar(&mut self, text: impl Into<Text>) {
        let Some(text) = checked(text.into()) else { return };
        self.update_connected(|| PlayerUpdate {
            action_bar: Some(text.clone()),
            ..Default::default()
        });
    }

    /// Replaces the player's title and subtitle.
    #[allow(dead_code)]
    pub fn title(&mut self, player: i32, title: Title) {
        let Some(title) = checked_title(title) else { return };
        let title = Some(TitleUpdate::Show(title));
        self.update(player, PlayerUpdate { title, ..Default::default() });
    }

    pub fn broadcast_title(&mut self, title: Title) {
        let Some(title) = checked_title(title) else { return };
        self.update_connected(|| PlayerUpdate {
            title: Some(TitleUpdate::Show(title.clone())),
            ..Default::default()
        });
    }

    #[allow(dead_code)]
    pub fn clear_title(&mut self, player: i32) {
        self.update(player, PlayerUpdate { title: Some(TitleUpdate::Clear), ..Default::default() });
    }

    fn update_connected(&mut self, update: impl Fn() -> PlayerUpdate) {
        // Index the members: `update` needs the context mutably.
        for index in 0..self.registry.members.len() {
            let player = self.registry.members[index];
            if self.connected(player) {
                self.update(player, update());
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

/// Games build their text, so out-of-bounds text is a server bug: it is dropped.
fn checked(text: Text) -> Option<Rc<Text>> {
    let valid = text.validate().is_ok();
    debug_assert!(valid, "invalid game text: {text:?}");
    valid.then(|| Rc::new(text))
}

fn checked_title(title: Title) -> Option<Rc<Title>> {
    let valid = title.validate().is_ok();
    debug_assert!(valid, "invalid game title: {title:?}");
    valid.then(|| Rc::new(title))
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

    pub fn create_sidebar(
        &mut self,
        layout: &'static SidebarLayout,
    ) -> Result<ObjectiveId, ScoreboardError> {
        self.registry.scoreboards.create_sidebar(None, layout)
    }

    pub fn create_personal_sidebar(
        &mut self,
        viewer: i32,
        layout: &'static SidebarLayout,
    ) -> Result<ObjectiveId, ScoreboardError> {
        self.player(viewer)?;
        self.registry.scoreboards.create_sidebar(Some(viewer), layout)
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
            ScoreHolder::Line(_) => return Err(ScoreboardError::InvalidKey),
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

    /// Publish one line from a game event. Formatting is immediate and uses
    /// stack storage; the caller's arguments are never retained or compared.
    pub fn update_line(
        &mut self,
        objective: ObjectiveId,
        line: LineId,
        text: std::fmt::Arguments<'_>,
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.update_line(objective, line, text)
    }

    /// Encode a shared event update once for several personal views of one layout.
    pub fn update_lines(
        &mut self,
        objectives: impl Iterator<Item = ObjectiveId> + Clone,
        line: LineId,
        text: std::fmt::Arguments<'_>,
    ) -> Result<(), ScoreboardError> {
        self.registry.scoreboards.update_lines(objectives, line, text)
    }
}
