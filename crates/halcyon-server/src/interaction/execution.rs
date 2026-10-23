//! Interaction dispatch and feedback; scheduling and acknowledgements stay with the owner.
use halcyon_world::{Aabb, BlockStateId, BlockView, World, blocks, opening};
use mc_protocol::packets::client::{InteractionHand, sound_category};

use super::{
    BlockInteraction, BlockUse, handle_break, jukebox_consumes, place_block, use_block, use_jukebox,
};
use crate::arena::{Arena, BlockPos};
use crate::effects::{EffectLog, Sound};
use crate::player::{PlayerState, PlayerUpdate};

pub struct Applied {
    pub cells: Vec<BlockPos>,
    feedback: Feedback,
}

enum Feedback {
    None,
    Sound { name: &'static str, pos: BlockPos, pitch: f32 },
    Break { pos: BlockPos, state: BlockStateId },
}

impl Applied {
    pub fn emit(&self, effects: &mut EffectLog, id: i32, hand: InteractionHand) {
        if !self.cells.is_empty() {
            effects.swing(id, hand, Some(id));
        }
        match self.feedback {
            Feedback::None => {}
            Feedback::Sound { name, pos, pitch } if !name.is_empty() => effects.sound(
                Sound { name, category: sound_category::BLOCK },
                pos.map(|v| f64::from(v) + 0.5),
                1.0,
                pitch,
                i64::from(id),
                Some(id),
            ),
            Feedback::Sound { .. } => {}
            Feedback::Break { pos, state } => effects.block_break(pos, state, Some(id)),
        }
    }
}

/// Rejection leaves terrain and inventory unchanged. Accepted inventory changes
/// join the same self update as the cells to restore before acknowledging.
pub fn execute(
    arena: &mut Arena,
    world: &World,
    player: &mut PlayerState,
    id: i32,
    action: BlockInteraction,
    occupants: &[Aabb],
    update: &mut PlayerUpdate,
) -> Option<Applied> {
    match action {
        BlockInteraction::ItemUse { .. } => None,
        BlockInteraction::Use(used) => execute_use(arena, world, player, used, occupants, update),
        BlockInteraction::Break(request) => {
            let [x, y, z] = request.pos;
            let before = arena.view(world).block_at(x, y, z);
            let cells = handle_break(arena, world, player, id, request)?;
            let feedback = match before {
                Some(state) if arena.view(world).block_at(x, y, z) != before => {
                    Feedback::Break { pos: request.pos, state }
                }
                _ => Feedback::None,
            };
            Some(Applied { cells, feedback })
        }
    }
}

fn execute_use(
    arena: &mut Arena,
    world: &World,
    player: &mut PlayerState,
    used: BlockUse,
    occupants: &[Aabb],
    update: &mut PlayerUpdate,
) -> Option<Applied> {
    let [x, y, z] = used.pos;
    let before = arena.view(world).block_at(x, y, z)?;
    let metadata = blocks::metadata(before);
    let bypass = player.movement.sneaking() && player.equipment[..2].iter().any(|s| !s.is_empty());
    if !bypass {
        if metadata.name == "minecraft:jukebox"
            && jukebox_consumes(arena, world, used.pos, player, used.main_hand)
        {
            let cells = use_jukebox(arena, world, player, used, update)?;
            return Some(Applied { cells, feedback: Feedback::None });
        }
        if opening(before).is_some() {
            let cells = use_block(arena, world, player, used)?;
            let after = arena.view(world).block_at(x, y, z)?;
            let feedback = if after == before {
                Feedback::None
            } else {
                Feedback::Sound {
                    name: if blocks::property(after, "open") == Some("true") {
                        metadata.open_sound
                    } else {
                        metadata.close_sound
                    },
                    pos: used.pos,
                    pitch: 1.0,
                }
            };
            return Some(Applied { cells, feedback });
        }
        if blocks::reserves_use(before) {
            return None;
        }
    }
    let cells = place_block(arena, world, player, used, occupants, update)?;
    let feedback = cells
        .first()
        .and_then(|&pos| {
            let state = arena.view(world).block_at(pos[0], pos[1], pos[2])?;
            Some(Feedback::Sound { name: blocks::metadata(state).place_sound, pos, pitch: 0.8 })
        })
        .unwrap_or(Feedback::None);
    Some(Applied { cells, feedback })
}
