use super::entities::{EntityData, MAX_PLAYING, songs};
use super::placement::hand_slot;
use super::{BlockUse, within_reach};
use crate::arena::{Arena, BlockPos, EditError};
use crate::player::{PlayerState, PlayerUpdate};
use halcyon_world::{BlockView, World, blocks};
use mc_protocol::types::{GameMode, ItemStack};

fn filled(arena: &Arena, world: &World, pos: BlockPos) -> bool {
    arena
        .block_entities
        .record(pos)
        .is_some_and(|r| matches!(&r.data,Some(EntityData::Jukebox(stack)) if !stack.is_empty()))
        || arena
            .view(world)
            .block_at(pos[0], pos[1], pos[2])
            .is_some_and(|s| blocks::property(s, "has_record") == Some("true"))
}

pub fn jukebox_consumes(
    arena: &Arena,
    world: &World,
    pos: BlockPos,
    player: &PlayerState,
    main: bool,
) -> bool {
    let held = player.inventory.stack(hand_slot(player, main));
    filled(arena, world, pos)
        || (!held.is_empty() && songs().iter().any(|song| song.item == held.item_id()))
}

pub fn use_jukebox(
    arena: &mut Arena,
    world: &World,
    player: &mut PlayerState,
    used: BlockUse,
    update: &mut PlayerUpdate,
) -> Option<Vec<BlockPos>> {
    if player.spectator()
        || !player.life.alive()
        || player.life.pending_teleport().is_some()
        || !within_reach(player, used.pos)
        || !used.cursor.iter().all(|v| v.is_finite() && (v - 0.5).abs() < 1.000_000_1)
    {
        return None;
    }
    let full = filled(arena, world, used.pos);
    let slot = hand_slot(player, used.main_hand);
    let mut stack = player.inventory.stack(slot).clone();
    let disc = if full {
        ItemStack::default()
    } else {
        if stack.is_empty() || !songs().iter().any(|song| song.item == stack.item_id()) {
            return None;
        }
        let mut disc = stack.clone();
        disc.take(disc.count().saturating_sub(1));
        disc
    };
    let cells = set_jukebox(arena, world, used.pos, disc, !full).ok()?;
    if !full && player.game_mode.current != GameMode::Creative {
        stack.take(1);
        let change = player.inventory.set_slot(slot, stack).expect("validated hand slot");
        player.inventory_changed(change, update);
    }
    Some(cells)
}

pub fn set_jukebox(
    arena: &mut Arena,
    world: &World,
    pos: BlockPos,
    disc: ItemStack,
    play: bool,
) -> Result<Vec<BlockPos>, EditError> {
    let state = arena
        .view(world)
        .block_at(pos[0], pos[1], pos[2])
        .filter(|s| blocks::metadata(*s).name == "minecraft:jukebox")
        .ok_or(EditError::Unsupported)?;
    if !arena.block_entities.room(pos) {
        return Err(EditError::Capacity);
    }
    if play
        && !disc.is_empty()
        && !arena.block_entities.playing.contains_key(&pos)
        && arena.block_entities.playing.len() >= MAX_PLAYING
    {
        return Err(EditError::Capacity);
    }
    if !disc.is_empty()
        && (disc.count() != 1 || !songs().iter().any(|song| song.item == disc.item_id()))
    {
        return Err(EditError::Unsupported);
    }
    let state =
        blocks::with(state, &[("has_record", if disc.is_empty() { "false" } else { "true" })])
            .ok_or(EditError::Unsupported)?;
    let mut cells = arena.edit_all(world, &[(pos, state)])?;
    arena.block_entities.set_disc(pos, state, disc, arena.tick(), play)?;
    if !cells.contains(&pos) {
        cells.push(pos);
    }
    Ok(cells)
}
