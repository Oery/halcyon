//! Budgeted, lazily validated weighted A*.
//!
//! Expanding a cell queues its geometric candidates at optimistic cost. A
//! candidate is validated by simulation only when it reaches the front of the
//! queue, then requeued at its true cost; most candidates never need a
//! simulation. A search keeps its state between calls so a bot can think
//! across several ticks.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use halcyon_world::BlockView;

use super::{Candidate, Cell, Edge, NavService, Validation};

/// Inflation of the heuristic: faster searches, somewhat longer routes.
const WEIGHT: f32 = 1.5;
/// Fixed extra cost of a jump over its duration: lining it up takes time.
pub const JUMP_COST: f32 = 8.0;
/// Rough cost of gaining one block of height, in ticks.
const RISE_COST: f32 = 15.0;
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
enum Via {
    Start,
    /// Reached through a candidate not yet validated.
    Pending(Cell, Candidate),
    Valid(Step),
}

#[derive(Clone, Copy)]
struct Open {
    f: f32,
    g: f32,
    cell: Cell,
    via: Via,
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

/// Estimated ticks between two cells; height dominates on a climb.
pub fn heuristic(from: Cell, goal: Cell) -> f32 {
    let dx = (goal[0] - from[0]) as f32;
    let dz = (goal[2] - from[2]) as f32;
    let rise = (goal[1] - from[1]) as f32;
    dx.hypot(dz) / 0.45 + rise.max(0.0) * RISE_COST
}

#[derive(Default)]
pub struct Search {
    open: BinaryHeap<Open>,
    /// Settled cells: best cost and how they were reached.
    closed: HashMap<Cell, (f32, Option<Step>)>,
    /// Best validated cost queued per cell. Unvalidated entries never prune
    /// one another: an optimistic estimate may still fail validation.
    queued: HashMap<Cell, f32>,
    goal: Option<Cell>,
    best: Option<(f32, Cell)>,
    expansions: usize,
}

impl Search {
    pub fn start(&mut self, start: Cell, goal: Cell) {
        self.open.clear();
        self.closed.clear();
        self.queued.clear();
        self.goal = Some(goal);
        self.best = Some((heuristic(start, goal), start));
        self.expansions = 0;
        self.open.push(Open {
            f: heuristic(start, goal) * WEIGHT,
            g: 0.0,
            cell: start,
            via: Via::Start,
        });
        self.queued.insert(start, 0.0);
    }

    pub fn is_active(&self) -> bool {
        self.goal.is_some()
    }

    pub fn cancel(&mut self) {
        self.goal = None;
        self.open.clear();
        self.closed.clear();
        self.queued.clear();
    }

    fn path_to(&self, mut cell: Cell) -> Vec<Step> {
        let mut path = Vec::new();
        while let Some((_, Some(step))) = self.closed.get(&cell) {
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

    fn push(&mut self, cell: Cell, g: f32, via: Via, goal: Cell) {
        if self.closed.contains_key(&cell)
            || self.queued.get(&cell).is_some_and(|&known| known <= g)
        {
            return;
        }
        if !matches!(via, Via::Pending(..)) {
            self.queued.insert(cell, g);
        }
        self.open.push(Open { f: g + heuristic(cell, goal) * WEIGHT, g, cell, via });
    }

    /// Advance by at most `budget` queue entries. `cost` adds a bot's own
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
            if self.closed.contains_key(&open.cell) {
                self.open.pop();
                continue;
            }
            match open.via {
                Via::Pending(from, candidate) => {
                    let edge = match nav.validate(view, from, &candidate) {
                        Validation::Busy => return SearchStatus::Running,
                        Validation::Invalid => {
                            self.open.pop();
                            continue;
                        }
                        Validation::Valid(edge) => edge,
                    };
                    self.open.pop();
                    let Some(&(from_g, _)) = self.closed.get(&from) else {
                        continue;
                    };
                    let mut g = from_g + f32::from(edge.ticks) + cost(from, &edge);
                    if edge.kind.is_jump() {
                        g += JUMP_COST;
                    }
                    self.push(open.cell, g, Via::Valid(Step { from, edge }), goal);
                }
                Via::Start | Via::Valid(_) => {
                    self.open.pop();
                    let step = match open.via {
                        Via::Valid(step) => Some(step),
                        _ => None,
                    };
                    self.closed.insert(open.cell, (open.g, step));
                    let h = heuristic(open.cell, goal);
                    if self.best.is_none_or(|(best, _)| h < best) {
                        self.best = Some((h, open.cell));
                    }
                    if open.cell == goal {
                        let path = self.path_to(goal);
                        return self.finish(SearchStatus::Found(path));
                    }
                    self.expansions += 1;
                    nav.graph.stats.expansions += 1;
                    if self.expansions > MAX_EXPANSIONS {
                        return self.partial();
                    }
                    let Some(candidates) = nav.candidates(view, open.cell) else {
                        continue;
                    };
                    for candidate in candidates.iter() {
                        let mut g = open.g + candidate.estimate;
                        if candidate.kind.is_jump() {
                            g += JUMP_COST;
                        }
                        self.push(candidate.to, g, Via::Pending(open.cell, *candidate), goal);
                    }
                }
            }
        }
        SearchStatus::Running
    }
}
