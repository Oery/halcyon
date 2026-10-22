//! Budgeted weighted A* over the lazily validated graph. A search keeps its
//! state between calls so a bot can think across several ticks.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use halcyon_world::BlockView;

use super::{Cell, Edge, EdgeKind, Expand, NavService};

/// Inflation of the heuristic: faster searches, slightly longer routes.
const WEIGHT: f32 = 1.5;
/// Fixed extra cost of a jump over its duration: lining it up takes time.
const JUMP_COST: f32 = 8.0;
pub const MAX_EXPANSIONS: usize = 60_000;

#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub from: Cell,
    pub edge: Edge,
}

#[derive(Debug)]
pub enum SearchStatus {
    Running,
    Found(Vec<Step>),
    /// The goal was not reached; this path leads to the closest cell found.
    Partial(Vec<Step>),
    Unreachable,
}

#[derive(Clone, Copy)]
struct Open {
    f: f32,
    g: f32,
    cell: Cell,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap on f, then prefer deeper nodes; cells break ties deterministically.
        other.f.total_cmp(&self.f).then(self.g.total_cmp(&other.g)).then(other.cell.cmp(&self.cell))
    }
}

/// Lower bound on ticks between two cells.
pub fn heuristic(from: Cell, goal: Cell) -> f32 {
    let dx = (goal[0] - from[0]) as f32;
    let dz = (goal[2] - from[2]) as f32;
    let rise = (goal[1] - from[1]) as f32;
    dx.hypot(dz) / 0.45 + rise.max(0.0) * 3.0
}

#[derive(Default)]
pub struct Search {
    open: BinaryHeap<Open>,
    parents: HashMap<Cell, (f32, Option<Step>)>,
    goal: Option<Cell>,
    start: Option<Cell>,
    best: Option<(f32, Cell)>,
    expansions: usize,
}

impl Search {
    pub fn start(&mut self, start: Cell, goal: Cell) {
        self.open.clear();
        self.parents.clear();
        self.goal = Some(goal);
        self.start = Some(start);
        self.best = Some((heuristic(start, goal), start));
        self.expansions = 0;
        self.open.push(Open { f: heuristic(start, goal) * WEIGHT, g: 0.0, cell: start });
        self.parents.insert(start, (0.0, None));
    }

    pub fn is_active(&self) -> bool {
        self.goal.is_some()
    }

    pub fn cancel(&mut self) {
        self.goal = None;
        self.open.clear();
        self.parents.clear();
    }

    fn path_to(&self, mut cell: Cell) -> Vec<Step> {
        let mut path = Vec::new();
        while let Some((_, Some(step))) = self.parents.get(&cell) {
            path.push(*step);
            cell = step.from;
        }
        path.reverse();
        path
    }

    fn finish(&mut self, status: SearchStatus) -> SearchStatus {
        self.goal = None;
        self.open.clear();
        status
    }

    fn partial(&mut self) -> SearchStatus {
        let path = self.best.map(|(_, cell)| self.path_to(cell)).unwrap_or_default();
        self.finish(if path.is_empty() {
            SearchStatus::Unreachable
        } else {
            SearchStatus::Partial(path)
        })
    }

    /// Advance by at most `budget` expansions. `cost` adds a bot's own
    /// preferences (risk aversion, remembered failures) to each edge.
    pub fn step(
        &mut self,
        nav: &mut NavService,
        view: &impl BlockView,
        budget: usize,
        cost: &impl Fn(Cell, &Edge) -> f32,
    ) -> SearchStatus {
        let Some(goal) = self.goal else {
            return SearchStatus::Unreachable;
        };
        for _ in 0..budget {
            let Some(open) = self.open.peek().copied() else {
                return self.partial();
            };
            if open.cell == goal {
                let path = self.path_to(goal);
                return self.finish(SearchStatus::Found(path));
            }
            if self.parents.get(&open.cell).is_some_and(|(g, _)| *g < open.g) {
                self.open.pop();
                continue;
            }
            let edges = match nav.expand(view, open.cell) {
                Expand::Ready(edges) => edges,
                Expand::Missing => {
                    self.open.pop();
                    continue;
                }
                Expand::Busy => return SearchStatus::Running,
            };
            self.open.pop();
            self.expansions += 1;
            nav.graph.stats.expansions += 1;
            if self.expansions > MAX_EXPANSIONS {
                return self.partial();
            }
            for edge in edges.iter() {
                let mut step = f32::from(edge.ticks) + cost(open.cell, edge);
                if edge.kind == EdgeKind::Jump {
                    step += JUMP_COST;
                }
                let g = open.g + step;
                if self.parents.get(&edge.to).is_some_and(|(known, _)| *known <= g) {
                    continue;
                }
                self.parents.insert(edge.to, (g, Some(Step { from: open.cell, edge: *edge })));
                let h = heuristic(edge.to, goal);
                if self.best.is_none_or(|(best, _)| h < best) {
                    self.best = Some((h, edge.to));
                }
                self.open.push(Open { f: g + h * WEIGHT, g, cell: edge.to });
            }
        }
        SearchStatus::Running
    }
}
