use super::placement::editable;
use super::{BlockBreak, break_block, break_cells, within_reach};
use crate::arena::{Arena, BlockPos};
use crate::inventory::InventorySlot;
use crate::player::PlayerState;
use halcyon_world::{BlockStateId, BlockView, FluidKind, World, blocks, fluid_info};
use mc_protocol::types::{GameMode, ItemStack};

pub struct Mining {
    pub pos: BlockPos,
    state: BlockStateId,
    generation: u64,
    slot: InventorySlot,
    tool: ItemStack,
    progress: f32,
    tick: u64,
    finish: bool,
}

impl Mining {
    pub fn hand_valid(&self, inventory: &crate::inventory::Inventory) -> bool {
        inventory.selected().inventory_slot() == self.slot
            && inventory.stack(self.slot) == &self.tool
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crack {
    pub pos: BlockPos,
    pub stage: u8,
}

fn speed(
    player: &PlayerState,
    view: &impl BlockView,
    state: BlockStateId,
    tool: &ItemStack,
) -> f32 {
    let block = blocks::metadata(state);
    if block.hardness < 0.0 {
        return 0.0;
    }
    if block.hardness == 0.0 {
        return 1.0;
    }
    let tool = block.tools.iter().find(|t| i32::from(t.item) == tool.item_id());
    let mut speed = tool.map_or(1.0, |t| t.speed);
    let p = player.movement.transform();
    let eye = p.y
        + match player.movement.pose() {
            mc_protocol::packets::client::PlayerPose::Standing => 1.62,
            mc_protocol::packets::client::PlayerPose::Crouching => 1.27,
            _ => 0.4,
        };
    let pos = [p.x.floor() as i32, eye.floor() as i32, p.z.floor() as i32];
    if let Some(state) = view.block_at(pos[0], pos[1], pos[2]) {
        let fluid = fluid_info(state);
        let water_above = view
            .block_at(pos[0], pos[1] + 1, pos[2])
            .is_some_and(|s| fluid_info(s).kind == FluidKind::Water);
        if fluid.kind == FluidKind::Water
            && eye
                < (pos[1] as f64
                    + if water_above {
                        1.0
                    } else {
                        f64::from(fluid.own_height())
                    })
        {
            speed /= 5.0;
        }
    }
    if !p.on_ground {
        speed /= 5.0;
    }
    let suitable = !block.requires_tool || tool.is_some_and(|t| t.correct);
    speed / block.hardness / if suitable { 30.0 } else { 100.0 }
}

fn valid(player: &PlayerState, view: &impl BlockView, mining: &Mining) -> bool {
    editable(player)
        && player.game_mode.current == GameMode::Survival
        && player.life.generation == mining.generation
        && within_reach(player, mining.pos)
        && player.inventory.selected().inventory_slot() == mining.slot
        && player.inventory.stack(mining.slot) == &mining.tool
        && view.block_at(mining.pos[0], mining.pos[1], mining.pos[2]) == Some(mining.state)
}

pub fn handle_break(
    arena: &mut Arena,
    base: &World,
    player: &mut PlayerState,
    id: i32,
    request: BlockBreak,
) -> Option<Vec<BlockPos>> {
    if player.game_mode.current == GameMode::Creative {
        player.mining = None;
        arena.cracks.remove(&id);
        return if request.action == 0 {
            break_block(arena, base, player, request)
        } else {
            Some(break_cells(&arena.view(base), request.pos))
        };
    }
    if request.action == 2 {
        player.mining = None;
        arena.cracks.remove(&id);
        return Some(break_cells(&arena.view(base), request.pos));
    }
    if !editable(player)
        || player.game_mode.current != GameMode::Survival
        || !within_reach(player, request.pos)
    {
        player.mining = None;
        arena.cracks.remove(&id);
        return None;
    }
    let view = arena.view(base);
    let state = view.block_at(request.pos[0], request.pos[1], request.pos[2])?;
    if blocks::placement(state).is_none() || blocks::metadata(state).hardness < 0.0 {
        return None;
    }
    match request.action {
        0 => {
            let slot = player.inventory.selected().inventory_slot();
            let tool = player.inventory.stack(slot).clone();
            let progress = speed(player, &view, state, &tool);
            player.mining = Some(Mining {
                pos: request.pos,
                state,
                generation: player.life.generation,
                slot,
                tool,
                progress,
                tick: arena.tick(),
                finish: progress >= 1.0,
            });
        }
        1 => {
            if !player
                .mining
                .as_ref()
                .is_some_and(|m| m.pos == request.pos && valid(player, &view, m))
            {
                return None;
            }
        }
        3 => {
            if !player
                .mining
                .as_ref()
                .is_some_and(|m| m.pos == request.pos && valid(player, &view, m))
            {
                return None;
            }
            player.mining.as_mut()?.finish = true;
        }
        _ => return None,
    }
    advance_mining(arena, base, player, id).or_else(|| Some(vec![request.pos]))
}

pub fn advance_mining(
    arena: &mut Arena,
    base: &World,
    player: &mut PlayerState,
    id: i32,
) -> Option<Vec<BlockPos>> {
    let Some(mut mining) = player.mining.take() else {
        arena.cracks.remove(&id);
        return None;
    };
    let view = arena.view(base);
    if !valid(player, &view, &mining) {
        arena.cracks.remove(&id);
        return None;
    }
    if mining.tick != arena.tick() {
        mining.progress += speed(player, &view, mining.state, &mining.tool);
        mining.tick = arena.tick();
    }
    if mining.progress >= 1.0 && mining.finish {
        let positions = break_cells(&view, mining.pos);
        let cells: Option<Vec<_>> = positions
            .iter()
            .map(|pos| Some((*pos, blocks::replacement(view.block_at(pos[0], pos[1], pos[2])?))))
            .collect();
        let result = arena.edit_all(base, &cells?).ok();
        arena.cracks.remove(&id);
        return result;
    }
    arena
        .cracks
        .insert(id, Crack { pos: mining.pos, stage: ((mining.progress * 10.0) as u8).min(9) });
    player.mining = Some(mining);
    None
}

/// Slow writers may defer a completion, but cannot retain an invalid crack.
pub fn cancel_invalid_mining(arena: &mut Arena, base: &World, player: &mut PlayerState, id: i32) {
    if player.mining.as_ref().is_none_or(|mining| !valid(player, &arena.view(base), mining)) {
        player.mining = None;
        arena.cracks.remove(&id);
    }
}
