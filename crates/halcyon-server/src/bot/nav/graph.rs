//! Lazily built, shared cache of standable cells, move candidates and their
//! validation results.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use halcyon_world::BlockView;

use super::moves::Candidate;
use super::stand::{Stand, stand};
use super::{Cell, Edge, EdgeKind, NavStats};
use crate::arena::Column;
use crate::lobby::Transform;
use crate::movement::MovementState;

pub struct NavGraph {
    pub(super) stands: HashMap<Cell, Option<Stand>>,
    pub(super) candidates: HashMap<Cell, Rc<[Candidate]>>,
    /// Keyed by move kind too: a failed stair walk must not hide the jump.
    pub(super) validated: HashMap<(Cell, Cell, EdgeKind), Option<Edge>>,
    /// Scratch for clearance probes.
    pub(super) probe: MovementState,
    /// Scratch for edge simulations.
    pub(super) sim: MovementState,
    pub stats: NavStats,
}

impl Default for NavGraph {
    fn default() -> Self {
        Self {
            stands: HashMap::new(),
            candidates: HashMap::new(),
            validated: HashMap::new(),
            probe: MovementState::new(Transform::default()),
            sim: MovementState::new(Transform::default()),
            stats: NavStats::default(),
        }
    }
}

fn column_of(cell: Cell) -> Column {
    (cell[0].div_euclid(16), cell[2].div_euclid(16))
}

impl NavGraph {
    pub fn stand(&mut self, view: &impl BlockView, cell: Cell) -> Option<Stand> {
        if let Some(found) = self.stands.get(&cell) {
            return *found;
        }
        let found = stand(view, &mut self.probe, cell);
        self.stands.insert(cell, found);
        if found.is_some() {
            self.stats.nodes += 1;
        }
        found
    }

    /// Unvalidated moves out of `cell`, or `None` if it is not standable.
    pub fn candidates(&mut self, view: &impl BlockView, cell: Cell) -> Option<Rc<[Candidate]>> {
        if let Some(found) = self.candidates.get(&cell) {
            return Some(found.clone());
        }
        let here = self.stand(view, cell)?;
        let found: Rc<[Candidate]> = self.build_candidates(view, cell, here).into();
        self.candidates.insert(cell, found.clone());
        Some(found)
    }

    /// A cached validation result, if `candidate` was already checked.
    pub fn checked(&self, from: Cell, candidate: &Candidate) -> Option<Option<Edge>> {
        self.validated.get(&(from, candidate.to, candidate.kind)).copied()
    }

    pub fn validate(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        candidate: &Candidate,
    ) -> Option<Edge> {
        if let Some(known) = self.checked(from, candidate) {
            return known;
        }
        let edge = self.stand(view, from).and_then(|here| {
            let start = Instant::now();
            let edge = self.validate_candidate(view, from, here, candidate);
            self.stats.build_time += start.elapsed();
            edge
        });
        self.validated.insert((from, candidate.to, candidate.kind), edge);
        if edge.is_some() {
            self.stats.edges += 1;
        }
        edge
    }

    /// Every validated edge out of `cell`, validating all candidates.
    #[cfg(test)]
    pub fn edges(&mut self, view: &impl BlockView, cell: Cell) -> Option<Rc<[Edge]>> {
        let candidates = self.candidates(view, cell)?;
        let edges: Vec<Edge> = candidates
            .iter()
            .filter_map(|candidate| self.validate(view, cell, candidate))
            .collect();
        Some(edges.into())
    }

    /// Forget everything that could depend on blocks in `dirty` columns. Moves
    /// reach at most five blocks, so neighbouring columns are dropped too.
    pub fn invalidate(&mut self, dirty: &HashSet<Column>) {
        let near = |cell: &Cell| {
            let (cx, cz) = column_of(*cell);
            (-1..=1).any(|dx| (-1..=1).any(|dz| dirty.contains(&(cx + dx, cz + dz))))
        };
        self.stands.retain(|cell, _| !near(cell));
        self.candidates.retain(|cell, _| !near(cell));
        self.validated.retain(|(from, to, _), _| !near(from) && !near(to));
        self.stats.nodes = self.stands.values().filter(|s| s.is_some()).count();
        self.stats.edges = self.validated.values().filter(|e| e.is_some()).count();
    }

    pub fn cached_cells(&self) -> usize {
        self.candidates.len()
    }
}
