//! Owner-validated hand block interactions, including Creative instant breaking.
use std::collections::VecDeque;
use std::sync::OnceLock;

use halcyon_world::{BlockStateId, BlockView, Opening, World, opening};
use mc_protocol::packets::client::{InteractionHand, PlayerPose};
use mc_protocol::types::GameMode;

use crate::arena::{Arena, BlockPos};
use crate::player::PlayerState;

const MAX_BLOCK_ACTIONS: usize = 8;
pub const BLOCK_ACTIONS_PER_STEP: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct BlockUse {
    pub sequence: i32,
    pub pos: BlockPos,
    /// Hit location relative to the block's minimum corner.
    pub cursor: [f32; 3],
    pub main_hand: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct BlockBreak {
    pub sequence: i32,
    pub pos: BlockPos,
}

#[derive(Clone, Copy, Debug)]
pub enum BlockInteraction {
    Use(BlockUse),
    Break(BlockBreak),
}

impl BlockInteraction {
    pub fn sequence(self) -> i32 {
        match self {
            Self::Use(used) => used.sequence,
            Self::Break(broken) => broken.sequence,
        }
    }

    pub fn pos(self) -> BlockPos {
        match self {
            Self::Use(used) => used.pos,
            Self::Break(broken) => broken.pos,
        }
    }

    pub fn hand(self) -> InteractionHand {
        match self {
            Self::Use(BlockUse { main_hand: false, .. }) => InteractionHand::Off,
            _ => InteractionHand::Main,
        }
    }
}

#[derive(Default)]
pub struct BlockQueue {
    pending: VecDeque<(u64, BlockInteraction)>,
}

impl BlockQueue {
    pub fn push(&mut self, generation: u64, used: BlockInteraction) -> bool {
        if self.pending.len() == MAX_BLOCK_ACTIONS {
            return false;
        }
        self.pending.push_back((generation, used));
        true
    }

    pub fn pop(&mut self) -> Option<(u64, BlockInteraction)> {
        self.pending.pop_front()
    }
}

fn within_reach(player: &PlayerState, pos: BlockPos) -> bool {
    let transform = player.movement.transform();
    let eye = match player.movement.pose() {
        PlayerPose::Standing => 1.62,
        PlayerPose::Crouching => 1.27,
        PlayerPose::Swimming => 0.4,
    };
    let [x, y, z] = pos;
    let gap = |v: f64, min: i32| (f64::from(min) - v).max(v - f64::from(min) - 1.0).max(0.0);
    let distance = gap(transform.x, x).powi(2)
        + gap(transform.y + eye, y).powi(2)
        + gap(transform.z, z).powi(2);
    // block_interaction_range plus vanilla's one-block server allowance.
    let reach: f64 = if player.game_mode.current == GameMode::Creative {
        6.0
    } else {
        5.5
    };
    distance < reach * reach
}

#[derive(Clone, Copy, Default)]
struct BreakState {
    replacement: Option<BlockStateId>,
    partner: Option<([i8; 3], BlockStateId)>,
}

static BREAK_STATES: OnceLock<Box<[BreakState]>> = OnceLock::new();

/// Shared immutable lookup, built once before gameplay, never by scanning on a break.
pub fn initialize() {
    break_states();
}

fn break_states() -> &'static [BreakState] {
    BREAK_STATES.get_or_init(|| {
        use halcyon_world::blockstates::{BLOCK_STATES, block_state_id};
        use halcyon_world::{FluidKind, fluid_info};
        let water = block_state_id("minecraft:water[level=0]").expect("water state") as u16;
        let lava = block_state_id("minecraft:lava[level=0]").expect("lava state") as u16;
        let mut states = vec![BreakState::default(); BLOCK_STATES.len()];
        for &(key, id) in BLOCK_STATES {
            let name = key.split('[').next().unwrap_or(key);
            if matches!(
                name,
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:void_air"
                    | "minecraft:water"
                    | "minecraft:lava"
                    | "minecraft:bubble_column"
            ) {
                continue;
            }
            let partner = if key.contains("half=upper") {
                Some(([0, -1, 0], key.replace("half=upper", "half=lower")))
            } else if key.contains("half=lower") {
                Some(([0, 1, 0], key.replace("half=lower", "half=upper")))
            } else if name.ends_with("_bed") {
                let mut offset = if key.contains("facing=north") {
                    [0, 0, -1]
                } else if key.contains("facing=south") {
                    [0, 0, 1]
                } else if key.contains("facing=west") {
                    [-1, 0, 0]
                } else {
                    [1, 0, 0]
                };
                if key.contains("part=head") {
                    offset = offset.map(|axis| -axis);
                    Some((offset, key.replace("part=head", "part=foot")))
                } else {
                    Some((offset, key.replace("part=foot", "part=head")))
                }
            } else {
                None
            };
            states[id as usize] = BreakState {
                replacement: Some(match fluid_info(id as u16).kind {
                    FluidKind::Empty => 0,
                    FluidKind::Water => water,
                    FluidKind::Lava => lava,
                }),
                partner: partner
                    .and_then(|(offset, key)| block_state_id(&key).map(|id| (offset, id as u16))),
            };
        }
        states.into_boxed_slice()
    })
}

/// The target plus a matching paired half; also used to restore rejected predictions.
pub fn break_cells(view: &impl BlockView, pos: BlockPos) -> Vec<BlockPos> {
    let mut cells = vec![pos];
    if let Some(state) = view.block_at(pos[0], pos[1], pos[2])
        && let Some((offset, expected)) = break_states()[state as usize].partner
    {
        let other = std::array::from_fn(|axis| pos[axis] + i32::from(offset[axis]));
        if view.block_at(other[0], other[1], other[2]) == Some(expected) {
            cells.push(other);
        }
    }
    cells
}

/// A Creative start-dig request removes its target immediately, with no drops.
pub fn break_block(
    arena: &mut Arena,
    base: &World,
    player: &PlayerState,
    broken: BlockBreak,
) -> Option<Vec<BlockPos>> {
    if player.game_mode.current != GameMode::Creative
        || !player.life.alive()
        || player.life.pending_teleport().is_some()
        || !within_reach(player, broken.pos)
    {
        return None;
    }
    let view = arena.view(base);
    let positions = break_cells(&view, broken.pos);
    let cells: Option<Vec<_>> = positions
        .iter()
        .map(|&pos| {
            let state = view.block_at(pos[0], pos[1], pos[2])?;
            Some((pos, break_states()[state as usize].replacement?))
        })
        .collect();
    arena.edit_all(base, &cells?).ok()?;
    Some(positions)
}

/// Returns the changed cells. A rejection leaves authority unchanged and the
/// caller restores the client's prediction.
pub fn use_block(
    arena: &mut Arena,
    base: &World,
    player: &PlayerState,
    used: BlockUse,
) -> Option<Vec<BlockPos>> {
    // Off-hand use never reaches an empty-hand block interaction; spectators
    // cannot operate blocks.
    if !used.main_hand
        || player.spectator()
        || !player.life.alive()
        || player.life.pending_teleport().is_some()
    {
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
    let [x, y, z] = used.pos;
    if !within_reach(player, used.pos) {
        return None;
    }
    let view = arena.view(base);
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
            let index = (f64::from(transform.yaw) / 90.0 + 0.5).floor() as i32 & 3;
            let direction = [2, 3, 0, 1][index as usize];
            let state = if !open && facing == (direction + 2) % 4 {
                reversed
            } else {
                toggled
            };
            vec![(used.pos, state)]
        }
    };
    arena.edit_all(base, &cells).ok()?;
    Some(cells.into_iter().map(|(pos, _)| pos).collect())
}

#[cfg(test)]
#[path = "tests/tests_interaction.rs"]
mod tests;
