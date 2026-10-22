//! Lazily built, shared cache of standable cells and validated edges.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use halcyon_world::BlockView;

use super::stand::{Stand, stand};
use super::{Cell, Edge, NavStats};
use crate::arena::Column;
use crate::lobby::Transform;
use crate::movement::MovementState;

pub struct NavGraph {
    stands: HashMap<Cell, Option<Stand>>,
    edges: HashMap<Cell, Rc<[Edge]>>,
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
            edges: HashMap::new(),
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

    pub fn cached_edges(&self, cell: Cell) -> Option<Rc<[Edge]>> {
        self.edges.get(&cell).cloned()
    }

    pub fn edges(&mut self, view: &impl BlockView, cell: Cell) -> Option<Rc<[Edge]>> {
        if let Some(edges) = self.edges.get(&cell) {
            return Some(edges.clone());
        }
        let from = self.stand(view, cell)?;
        let start = Instant::now();
        let edges: Rc<[Edge]> = self.build_edges(view, cell, from).into();
        self.stats.build_time += start.elapsed();
        self.stats.edges += edges.len();
        self.edges.insert(cell, edges.clone());
        Some(edges)
    }

    /// Forget everything that could depend on blocks in `dirty` columns. Edges
    /// reach at most five blocks, so neighbouring columns are dropped too.
    pub fn invalidate(&mut self, dirty: &HashSet<Column>) {
        let near = |cell: &Cell| {
            let (cx, cz) = column_of(*cell);
            (-1..=1).any(|dx| (-1..=1).any(|dz| dirty.contains(&(cx + dx, cz + dz))))
        };
        self.stands.retain(|cell, _| !near(cell));
        let before = self.edges.len();
        self.edges.retain(|cell, _| !near(cell));
        if before != self.edges.len() {
            self.stats.nodes = self.stands.values().filter(|s| s.is_some()).count();
            self.stats.edges = self.edges.values().map(|e| e.len()).sum();
        }
    }

    pub fn cached_cells(&self) -> usize {
        self.edges.len()
    }
}
