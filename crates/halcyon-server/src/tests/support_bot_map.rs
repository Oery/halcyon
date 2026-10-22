//! Sparse synthetic terrain for bot tests: air everywhere except placed blocks.

use std::collections::HashMap;

use halcyon_world::blockstates::block_state_id;
use halcyon_world::{BlockStateId, BlockView, MAX_BLOCK_Y, MIN_BLOCK_Y};

use crate::arena::BlockPos;

pub fn id(key: &str) -> BlockStateId {
    block_state_id(key).unwrap_or_else(|| panic!("unknown block state {key}")) as BlockStateId
}

pub fn stone() -> BlockStateId {
    id("minecraft:stone")
}

#[derive(Default)]
pub struct MapView {
    blocks: HashMap<BlockPos, BlockStateId>,
}

impl BlockView for MapView {
    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        if !(MIN_BLOCK_Y..MAX_BLOCK_Y).contains(&y) || !self.contains_column(x >> 4, z >> 4) {
            return None;
        }
        Some(self.blocks.get(&[x, y, z]).copied().unwrap_or(0))
    }

    fn contains_column(&self, x: i32, z: i32) -> bool {
        (-8..8).contains(&x) && (-8..8).contains(&z)
    }
}

impl MapView {
    pub fn set(&mut self, pos: BlockPos, state: BlockStateId) -> &mut Self {
        if state == 0 {
            self.blocks.remove(&pos);
        } else {
            self.blocks.insert(pos, state);
        }
        self
    }

    /// Stone floor whose top is at `y + 1`, spanning the inclusive x/z ranges.
    pub fn floor(&mut self, x: [i32; 2], y: i32, z: [i32; 2]) -> &mut Self {
        for bx in x[0]..=x[1] {
            for bz in z[0]..=z[1] {
                self.set([bx, y, bz], stone());
            }
        }
        self
    }

    /// A one-block stone pillar from y = 0 up to `top` inclusive.
    pub fn pillar(&mut self, x: i32, top: i32, z: i32) -> &mut Self {
        for y in 0..=top {
            self.set([x, y, z], stone());
        }
        self
    }
}
