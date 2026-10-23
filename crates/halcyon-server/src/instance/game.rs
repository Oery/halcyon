//! The owner-side facade a game uses to read and direct its instance's players.

use std::rc::Rc;

use super::session::Input;
use super::{InstanceId, Registry, Transform};
use crate::game::Destination;
use crate::lifecycle::PlayerRules;
use crate::player::{PlayerAction, PlayerUpdate};

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
    pub fn players(&self) -> Vec<i32> {
        let mut ids: Vec<_> = self
            .registry
            .players
            .iter()
            .filter(|(_, player)| !player.input.closed.get())
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
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
        for player in self.players() {
            self.update(
                player,
                PlayerUpdate { messages: vec![text.clone()], ..Default::default() },
            );
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
