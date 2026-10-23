//! Terrain navigation shared by every bot in a lobby.
//!
//! Nodes are standable cells; edges are movement primitives (walks, drops,
//! jumps, ladder climbs). Every non-trivial edge is validated by running the
//! real player physics under the same controller bots use at runtime, so the
//! graph never promises a move the physics cannot perform. Edges are built
//! lazily as searches expand them and are shared through one cache.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use halcyon_world::BlockView;

use crate::arena::{Arena, BlockPos, Column};

pub mod follow;
pub mod graph;
pub mod moves;
pub mod search;
pub mod stand;

pub use graph::NavGraph;
pub use moves::Candidate;
pub use stand::{Stand, StandKind};

pub type Cell = BlockPos;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// Steps between neighbouring cells, including step-ups and ladder exits.
    Walk,
    /// Walks off a ledge onto lower ground.
    Drop,
    /// Jumps over a gap or onto higher ground.
    Jump,
    /// Climbs a ladder one cell up.
    Climb,
    /// Jumps onto a slime block (`Edge::via`) and rebounds onto `to`.
    Bounce,
}

impl EdgeKind {
    /// Committed airborne maneuvers: lined up, then jumped.
    pub fn is_jump(self) -> bool {
        matches!(self, EdgeKind::Jump | EdgeKind::Bounce)
    }
}

/// How a validated edge is executed; replayed verbatim at runtime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Distance stepped back from the source centre before a jump.
    pub runup: f32,
    /// Distance from the source centre along the jump line at which to jump.
    pub takeoff: f32,
    pub sprint: bool,
    /// Sprint-jump once along the run-up first: each sprint jump adds speed
    /// that landing does not take away, enough for the longest level jumps.
    pub hop: bool,
    /// For walks: aim this far past the target centre, in world x/z. Around
    /// a wall's corner, leaning along the wall resolves that axis first and
    /// slides onto the next block instead of into the wall.
    pub lean: [f32; 2],
}

impl Plan {
    pub const WALK: Plan =
        Plan { runup: 0.0, takeoff: f32::INFINITY, sprint: false, hop: false, lean: [0.0; 2] };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub to: Cell,
    pub to_feet: f64,
    pub kind: EdgeKind,
    pub plan: Plan,
    /// Ticks taken from rest under ideal hands.
    pub ticks: u16,
    /// Fraction of perturbed executions that still succeed.
    pub robustness: f32,
    /// For bounces: the slime cell landed on first, and its feet height.
    pub via: Option<(Cell, f64)>,
    /// Fall damage taken on landing, in half hearts.
    pub damage: f32,
    /// A jump that ends by grabbing a ladder rather than landing on a floor:
    /// the ladder's facing, away from the wall that must be pushed to climb.
    pub onto_ladder: Option<u8>,
}

/// Closed hand-operated doors and fence gates, mapped to their open state.
/// Trapdoors are left alone: opening one removes a floor as often as a wall.
fn opened(id: halcyon_world::BlockStateId) -> Option<halcyon_world::BlockStateId> {
    use halcyon_world::{Opening, opening};
    static OPENED: std::sync::OnceLock<
        HashMap<halcyon_world::BlockStateId, halcyon_world::BlockStateId>,
    > = std::sync::OnceLock::new();
    OPENED
        .get_or_init(|| {
            halcyon_world::blockstates::BLOCK_STATES
                .iter()
                .filter(|(key, _)| key.contains("open=false"))
                .filter_map(|&(_, id)| {
                    let id = id as halcyon_world::BlockStateId;
                    match opening(id)? {
                        Opening::Door { toggled, .. } | Opening::FenceGate { toggled, .. } => {
                            Some((id, toggled))
                        }
                        Opening::Trapdoor { .. } => None,
                    }
                })
                .collect()
        })
        .get(&id)
        .copied()
}

/// Whether `id` is a closed door or fence gate a bot can open by hand.
pub fn closed_door(id: halcyon_world::BlockStateId) -> bool {
    opened(id).is_some()
}

/// The world as bots plan it: closed doors and fence gates already open,
/// since a bot opens them when it gets there.
pub struct OpenDoors<'a, V>(pub &'a V);

impl<V: BlockView> BlockView for OpenDoors<'_, V> {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<halcyon_world::BlockStateId> {
        self.0.block_at(x, y, z).map(|id| opened(id).unwrap_or(id))
    }

    fn contains_column(&self, x: i32, z: i32) -> bool {
        self.0.contains_column(x, z)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NavStats {
    pub nodes: usize,
    pub edges: usize,
    pub simulations: u64,
    pub simulated_ticks: u64,
    pub build_time: Duration,
    pub expansions: u64,
}

/// Validation budget. Searches pause when it is spent and resume later.
#[derive(Clone, Copy, Debug)]
pub enum Budget {
    /// Headless scenarios and tests.
    #[allow(dead_code)]
    Unlimited,
    PerWindow {
        limit: Duration,
        window: Duration,
    },
}

pub struct NavService {
    pub graph: NavGraph,
    budget: Budget,
    window_start: Instant,
    spent: Duration,
    arena_revision: Option<u64>,
    columns: HashMap<Column, u64>,
    pub goal: Option<Cell>,
    /// Intermediate goals in course order, before `goal`.
    pub checkpoints: Vec<Cell>,
}

pub enum Validation {
    Valid(Edge),
    Invalid,
    /// The validation budget is spent; try again later.
    Busy,
}

impl NavService {
    pub fn new(budget: Budget) -> Self {
        Self {
            graph: NavGraph::default(),
            budget,
            window_start: Instant::now(),
            spent: Duration::ZERO,
            arena_revision: None,
            columns: HashMap::new(),
            goal: None,
            checkpoints: Vec::new(),
        }
    }

    pub fn stats(&self) -> NavStats {
        self.graph.stats
    }

    fn has_budget(&mut self) -> bool {
        match self.budget {
            Budget::Unlimited => true,
            Budget::PerWindow { limit, window } => {
                if self.window_start.elapsed() >= window {
                    self.window_start = Instant::now();
                    self.spent = Duration::ZERO;
                }
                self.spent < limit
            }
        }
    }

    pub fn stand(&mut self, view: &impl BlockView, cell: Cell) -> Option<Stand> {
        self.graph.stand(view, cell)
    }

    /// Unvalidated moves out of `cell`; `None` if it is not standable.
    pub fn candidates(&mut self, view: &impl BlockView, cell: Cell) -> Option<Rc<[Candidate]>> {
        let start = Instant::now();
        let found = self.graph.candidates(view, cell);
        self.spent += start.elapsed();
        found
    }

    /// Validate a candidate within the shared budget.
    pub fn validate(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        candidate: &Candidate,
    ) -> Validation {
        let known = match self.graph.checked(from, candidate) {
            Some(known) => known,
            None => {
                if !self.has_budget() {
                    return Validation::Busy;
                }
                let start = Instant::now();
                let edge = self.graph.validate(view, from, candidate);
                self.spent += start.elapsed();
                edge
            }
        };
        known.map_or(Validation::Invalid, Validation::Valid)
    }

    /// Drop cached navigation around columns edited since the last call.
    pub fn sync(&mut self, arena: &Arena) {
        if self.arena_revision == Some(arena.revision) {
            return;
        }
        self.arena_revision = Some(arena.revision);
        let mut dirty = HashSet::new();
        for (&column, changed) in &arena.columns {
            if self.columns.get(&column) != Some(&changed.revision) {
                dirty.insert(column);
            }
        }
        // A column that lost all of its edits reverted to the base world.
        dirty.extend(self.columns.keys().filter(|c| !arena.columns.contains_key(c)).copied());
        self.columns = arena.columns.iter().map(|(&c, changed)| (c, changed.revision)).collect();
        if !dirty.is_empty() {
            self.graph.invalidate(&dirty);
        }
    }

    /// Gold (light weighted) pressure plates within `radius` blocks of
    /// `around`, the course's checkpoints. Ordered by height, ties broken
    /// by distance from the previous checkpoint.
    pub fn detect_checkpoints(
        &mut self,
        view: &impl BlockView,
        around: Cell,
        radius: i32,
    ) -> Vec<Cell> {
        use halcyon_world::{MAX_BLOCK_Y, MIN_BLOCK_Y};
        let plates: HashSet<u16> = halcyon_world::blockstates::BLOCK_STATES
            .iter()
            .filter(|(key, _)| key.starts_with("minecraft:light_weighted_pressure_plate"))
            .map(|&(_, id)| id as u16)
            .collect();
        let mut found = Vec::new();
        for x in around[0] - radius..=around[0] + radius {
            for z in around[2] - radius..=around[2] + radius {
                if !view.contains_column(x >> 4, z >> 4) {
                    continue;
                }
                for y in MIN_BLOCK_Y..MAX_BLOCK_Y {
                    if view.block_at(x, y, z).is_some_and(|id| plates.contains(&id))
                        && self.graph.stand(view, [x, y, z]).is_some()
                    {
                        found.push([x, y, z]);
                    }
                }
            }
        }
        found.sort_by_key(|cell| cell[1]);
        let mut ordered: Vec<Cell> = Vec::with_capacity(found.len());
        let mut previous = around;
        while !found.is_empty() {
            let height = found[0][1];
            let (index, _) = found
                .iter()
                .enumerate()
                .take_while(|(_, cell)| cell[1] == height)
                .min_by_key(|(_, cell)| {
                    (cell[0] - previous[0]).pow(2) + (cell[2] - previous[2]).pow(2)
                })
                .expect("found is not empty");
            previous = found.remove(index);
            ordered.push(previous);
        }
        ordered
    }

    /// The highest standable cell within `radius` blocks of `around`.
    pub fn detect_goal(
        &mut self,
        view: &impl BlockView,
        around: Cell,
        radius: i32,
    ) -> Option<Cell> {
        use halcyon_world::{MAX_BLOCK_Y, MIN_BLOCK_Y, state_info};
        let mut best: Option<Cell> = None;
        for x in around[0] - radius..=around[0] + radius {
            for z in around[2] - radius..=around[2] + radius {
                if !view.contains_column(x >> 4, z >> 4) {
                    continue;
                }
                let floor = best.map_or(MIN_BLOCK_Y, |best| best[1]);
                for y in (floor..MAX_BLOCK_Y - 1).rev() {
                    let Some(id) = view.block_at(x, y, z) else { continue };
                    if state_info(id).collision_boxes().is_empty() {
                        continue;
                    }
                    // Surfaces on top of this block, then the block itself (slabs).
                    let found = [[x, y + 1, z], [x, y, z]]
                        .into_iter()
                        .find(|&cell| self.graph.stand(view, cell).is_some());
                    if let Some(cell) = found
                        && best.is_none_or(|best| cell[1] > best[1])
                    {
                        best = Some(cell);
                    }
                    break;
                }
            }
        }
        best
    }
}

#[cfg(test)]
#[path = "../../tests/tests_bot_nav.rs"]
mod tests;

#[cfg(test)]
#[path = "../../bench/bot_nav.rs"]
mod benchmarks;
