//! Owner-validated hand use of doors, trapdoors and fence gates.
use std::collections::VecDeque;

use halcyon_world::{BlockStateId, BlockView, Opening, World, opening};
use mc_protocol::packets::client::PlayerPose;

use crate::arena::{Arena, BlockPos};
use crate::player::PlayerState;

const MAX_USES: usize = 8;
pub const USES_PER_STEP: usize = 4;
/// Survival `block_interaction_range` plus vanilla's one-block server allowance.
const MAX_REACH_SQ: f64 = (4.5 + 1.0) * (4.5 + 1.0);

#[derive(Clone, Copy, Debug)]
pub struct BlockUse {
    pub sequence: i32,
    pub pos: BlockPos,
    /// Hit location relative to the block's minimum corner.
    pub cursor: [f32; 3],
    pub main_hand: bool,
}

#[derive(Default)]
pub struct UseQueue {
    pending: VecDeque<BlockUse>,
}

impl UseQueue {
    pub fn push(&mut self, used: BlockUse) -> bool {
        if self.pending.len() == MAX_USES {
            return false;
        }
        self.pending.push_back(used);
        true
    }

    pub fn pop(&mut self) -> Option<BlockUse> {
        self.pending.pop_front()
    }
}

/// Returns the changed cells. A rejection leaves authority unchanged and the
/// caller restores the client's prediction.
pub fn use_block(
    arena: &mut Arena,
    base: &World,
    player: &PlayerState,
    used: BlockUse,
) -> Option<Vec<BlockPos>> {
    // Off-hand use never reaches an empty-hand block interaction.
    if !used.main_hand || !player.life.alive() || player.life.pending_teleport().is_some() {
        return None;
    }
    if !used.cursor.iter().all(|c| (c - 0.5).abs() < 1.000_000_1) {
        return None;
    }
    // Sneaking with any held item skips the block and uses the item instead.
    if player.movement.sneaking() && player.equipment[..2].iter().any(|s| !s.is_empty()) {
        return None;
    }
    let transform = player.movement.transform();
    let eye = match player.movement.pose() {
        PlayerPose::Standing => 1.62,
        PlayerPose::Crouching => 1.27,
        PlayerPose::Swimming => 0.4,
    };
    let [x, y, z] = used.pos;
    let gap = |v: f64, min: i32| (f64::from(min) - v).max(v - f64::from(min) - 1.0).max(0.0);
    let distance = gap(transform.x, x).powi(2)
        + gap(transform.y + eye, y).powi(2)
        + gap(transform.z, z).powi(2);
    if distance >= MAX_REACH_SQ {
        return None;
    }
    let cells = toggled_cells(&arena.view(base), used.pos, transform.yaw)?;
    arena.edit_all(base, &cells).ok()?;
    Some(cells.into_iter().map(|(pos, _)| pos).collect())
}

/// The cells and states that using the opening at `pos` by hand would set, for
/// a user facing `yaw`; `None` if nothing there opens by hand.
pub fn toggled_cells(
    view: &impl BlockView,
    pos: BlockPos,
    yaw: f32,
) -> Option<Vec<(BlockPos, BlockStateId)>> {
    let [x, y, z] = pos;
    let used = BlockUse { sequence: 0, pos, cursor: [0.5; 3], main_hand: true };
    let cells: Vec<(BlockPos, BlockStateId)> = match opening(view.block_at(x, y, z)?)? {
        Opening::Door { toggled, paired, upper } => {
            let partner = [x, if upper { y - 1 } else { y + 1 }, z];
            // Vanilla replaces a half without its matching partner by air. Both
            // halves must agree so the partner can simply mirror this one.
            match opening(view.block_at(partner[0], partner[1], partner[2])?)? {
                Opening::Door { paired: mirror, upper: other, .. }
                    if other != upper && mirror == toggled =>
                {
                    vec![(used.pos, toggled), (partner, paired)]
                }
                _ => return None,
            }
        }
        Opening::Trapdoor { toggled } => vec![(used.pos, toggled)],
        Opening::FenceGate { toggled, open, facing, reversed } => {
            // Direction.fromYRot in N, E, S, W order.
            let index = (f64::from(yaw) / 90.0 + 0.5).floor() as i32 & 3;
            let direction = [2, 3, 0, 1][index as usize];
            let state = if !open && facing == (direction + 2) % 4 {
                reversed
            } else {
                toggled
            };
            vec![(used.pos, state)]
        }
    };
    Some(cells)
}

#[cfg(test)]
#[path = "tests/tests_interaction.rs"]
mod tests;
