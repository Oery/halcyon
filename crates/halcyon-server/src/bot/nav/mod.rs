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
pub use stand::{Stand, StandKind};

pub type Cell = BlockPos;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Steps between neighbouring cells, including step-ups and ladder exits.
    Walk,
    /// Walks off a ledge onto lower ground.
    Drop,
    /// Jumps over a gap or onto higher ground.
    Jump,
    /// Climbs a ladder one cell up.
    Climb,
}

/// How a validated edge is executed; replayed verbatim at runtime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Distance stepped back from the source centre before a jump.
    pub runup: f32,
    /// Distance from the source centre along the jump line at which to jump.
    pub takeoff: f32,
    pub sprint: bool,
}

impl Plan {
    pub const WALK: Plan = Plan { runup: 0.0, takeoff: f32::INFINITY, sprint: false };
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
}

pub enum Expand {
    Ready(Rc<[Edge]>),
    /// Not a standable cell.
    Missing,
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

    /// Outgoing edges of `cell`, validating them first if needed.
    pub fn expand(&mut self, view: &impl BlockView, cell: Cell) -> Expand {
        if let Some(edges) = self.graph.cached_edges(cell) {
            return Expand::Ready(edges);
        }
        if self.graph.stand(view, cell).is_none() {
            return Expand::Missing;
        }
        if !self.has_budget() {
            return Expand::Busy;
        }
        let start = Instant::now();
        let edges = self.graph.edges(view, cell);
        self.spent += start.elapsed();
        edges.map_or(Expand::Missing, Expand::Ready)
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
