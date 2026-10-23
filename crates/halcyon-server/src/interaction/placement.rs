use super::{BlockUse, within_reach};
use crate::arena::{Arena, BlockPos};
use crate::inventory::InventorySlot;
use crate::player::{PlayerState, PlayerUpdate};
use halcyon_world::{
    Aabb, BlockStateId, BlockView, FluidKind, World, blocks::*, fluid_info, state_info,
};
use mc_protocol::data::items::ITEMS;
use mc_protocol::types::GameMode;
use std::sync::OnceLock;

pub fn placement_item(item: i32) -> Option<BlockStateId> {
    static ITEMS_TO_BLOCKS: OnceLock<Box<[Option<BlockStateId>]>> = OnceLock::new();
    ITEMS_TO_BLOCKS
        .get_or_init(|| {
            ITEMS
                .iter()
                .map(|(name, ..)| {
                    named(name).filter(|b| placement(b.default).is_some()).map(|b| b.default)
                })
                .collect()
        })
        .get(usize::try_from(item).ok()?)
        .copied()
        .flatten()
}

pub fn body(player: &PlayerState) -> Aabb {
    let p = player.movement.transform();
    let height = match player.movement.pose() {
        mc_protocol::packets::client::PlayerPose::Standing => 1.8,
        mc_protocol::packets::client::PlayerPose::Crouching => 1.5,
        mc_protocol::packets::client::PlayerPose::Swimming => 0.6,
    };
    Aabb { min: [p.x - 0.3, p.y, p.z - 0.3], max: [p.x + 0.3, p.y + height, p.z + 0.3] }
}

pub fn hand_slot(player: &PlayerState, main: bool) -> InventorySlot {
    if main {
        player.inventory.selected().inventory_slot()
    } else {
        InventorySlot::OFFHAND
    }
}

pub fn editable(player: &PlayerState) -> bool {
    player.life.alive()
        && player.life.pending_teleport().is_none()
        && match player.game_mode.current {
            GameMode::Creative => player.life.rules.block_edits.creative,
            GameMode::Survival => player.life.rules.block_edits.survival,
            _ => false,
        }
}

pub fn place_block(
    arena: &mut Arena,
    base: &World,
    player: &mut PlayerState,
    used: BlockUse,
    occupants: &[Aabb],
    update: &mut PlayerUpdate,
) -> Option<Vec<BlockPos>> {
    if !editable(player)
        || used.face > 5
        || !within_reach(player, used.pos)
        || !used.cursor.iter().all(|v| v.is_finite() && (v - 0.5).abs() < 1.000_000_1)
    {
        return None;
    }
    let slot = hand_slot(player, used.main_hand);
    let mut stack = player.inventory.stack(slot).clone();
    if stack.is_empty() {
        return None;
    }
    let initial = placement_item(stack.item_id())?;
    let view = arena.view(base);
    let cells = placement_cells(&view, initial, used, player.movement.transform().yaw)?;
    let cells = updates(&view, &cells)?;
    // A placement whose support closure removes the placed block is rejected.
    let target = cells.first()?.0;
    if cells.iter().find(|(p, _)| *p == target).is_none_or(|(_, s)| {
        metadata(*s).name != metadata(initial).name
            && !(placement(initial) == Some(Placement::Sign)
                && placement(*s) == Some(Placement::Sign))
    }) {
        return None;
    }
    for &(pos, state) in &cells {
        if view.block_at(pos[0], pos[1], pos[2]) == Some(state) {
            continue;
        }
        for shape in state_info(state).collision_boxes() {
            let bounds = Aabb {
                min: std::array::from_fn(|i| f64::from(pos[i]) + shape.min[i]),
                max: std::array::from_fn(|i| f64::from(pos[i]) + shape.max[i]),
            };
            if occupants.iter().any(|b| intersects(bounds, *b))
                || intersects(bounds, body(player))
                || arena.falling.iter().any(|e| {
                    intersects(
                        bounds,
                        Aabb {
                            min: [e.position[0] - 0.49, e.position[1], e.position[2] - 0.49],
                            max: [e.position[0] + 0.49, e.position[1] + 0.98, e.position[2] + 0.49],
                        },
                    )
                })
            {
                return None;
            }
        }
    }
    let changed = arena.edit_all(base, &cells).ok()?;
    if changed.is_empty() {
        return None;
    }
    if player.game_mode.current != GameMode::Creative {
        stack.take(1);
        let change = player.inventory.set_slot(slot, stack).expect("validated held slot");
        player.inventory_changed(change, update);
    }
    Some(changed)
}

fn intersects(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|i| a.max[i] > b.min[i] + 1e-7 && a.min[i] < b.max[i] - 1e-7)
}

pub(super) fn placement_cells(
    view: &impl BlockView,
    initial: BlockStateId,
    used: BlockUse,
    yaw: f32,
) -> Option<Vec<(BlockPos, BlockStateId)>> {
    let kind = placement(initial)?;
    let clicked = view.block_at(used.pos[0], used.pos[1], used.pos[2])?;
    let merge = |state| {
        kind == Placement::Slab
            && metadata(state).name == metadata(initial).name
            && property(state, "type") != Some("double")
    };
    let clicked_merge = merge(clicked)
        && match property(clicked, "type")? {
            "bottom" => used.face == 1 || (used.face > 1 && used.cursor[1] > 0.5),
            "top" => used.face == 0 || (used.face > 1 && used.cursor[1] <= 0.5),
            _ => false,
        };
    let pos = if replaceable(clicked) || clicked_merge {
        used.pos
    } else {
        offset(used.pos, used.face)
    };
    let old = view.block_at(pos[0], pos[1], pos[2])?;
    if merge(old) {
        return Some(vec![(pos, with(old, &[("type", "double"), ("waterlogged", "false")])?)]);
    }
    if !replaceable(old) {
        return None;
    }
    let fluid = fluid_info(old);
    let water = fluid.kind == FluidKind::Water && fluid.amount == 8 && !fluid.falling;
    let mut state = with(initial, &[("waterlogged", if water { "true" } else { "false" })])?;
    let facing = facing(yaw);
    let axis = match used.face {
        0 | 1 => "y",
        2 | 3 => "z",
        _ => "x",
    };
    let half = if used.face == 0 || (used.face > 1 && used.cursor[1] > 0.5) {
        "top"
    } else {
        "bottom"
    };
    match kind {
        Placement::Axis => state = with(state, &[("axis", axis)])?,
        Placement::Slab => state = with(state, &[("type", half)])?,
        Placement::Stairs => {
            state = with(state, &[("facing", facing), ("half", half), ("shape", "straight")])?
        }
        Placement::Gate => {
            state = with(state, &[("facing", facing), ("open", "false"), ("powered", "false")])?
        }
        Placement::Trapdoor => {
            let facing = if used.face > 1 {
                match used.face {
                    2 => "north",
                    3 => "south",
                    4 => "west",
                    _ => "east",
                }
            } else {
                facing
            };
            state = with(
                state,
                &[("facing", facing), ("half", half), ("open", "false"), ("powered", "false")],
            )?;
        }
        Placement::Door => {
            let upper = offset(pos, 1);
            if !view.block_at(upper[0], upper[1], upper[2]).is_some_and(replaceable) {
                return None;
            }
            let index = HORIZONTAL.iter().position(|s| *s == facing)?;
            let left = horizontal(HORIZONTAL[(index + 3) % 4]);
            let right = left ^ 1;
            let full = |p: BlockPos| {
                view.block_at(p[0], p[1], p[2])
                    .is_some_and(|s| state_info(s).kind == halcyon_world::BlockKind::Full)
            };
            let score = |d| i32::from(full(offset(pos, d))) + i32::from(full(offset(upper, d)));
            let neighbor_door = |d| {
                let p = offset(pos, d);
                view.block_at(p[0], p[1], p[2]).is_some_and(|s| {
                    placement(s) == Some(Placement::Door) && property(s, "half") == Some("lower")
                })
            };
            let hinge = if neighbor_door(left) && !neighbor_door(right)
                || score(left) > score(right)
            {
                "right"
            } else if neighbor_door(right) && !neighbor_door(left) || score(right) > score(left) {
                "left"
            } else {
                let [x, _, z] = used.cursor;
                if match facing {
                    "north" => x < 0.5,
                    "south" => x > 0.5,
                    "west" => z > 0.5,
                    _ => z < 0.5,
                } {
                    "right"
                } else {
                    "left"
                }
            };
            state = with(
                state,
                &[
                    ("facing", facing),
                    ("half", "lower"),
                    ("hinge", hinge),
                    ("open", "false"),
                    ("powered", "false"),
                ],
            )?;
            return Some(vec![(pos, state), (upper, with(state, &[("half", "upper")])?)]);
        }
        Placement::Sign => {
            let name = metadata(initial).name;
            let hanging = name.ends_with("_hanging_sign");
            let block = if used.face > 1 {
                if hanging {
                    name.replace("_hanging_sign", "_wall_hanging_sign")
                } else {
                    name.replace("_sign", "_wall_sign")
                }
            } else {
                name.to_owned()
            };
            if (hanging && used.face == 1) || (!hanging && used.face == 0) {
                return None;
            }
            let rotation = (((f64::from(yaw) + 180.0) * 16.0 / 360.0 + 0.5).floor() as i32)
                .rem_euclid(16)
                .to_string();
            let facing = match used.face {
                2 => "north",
                3 => "south",
                4 => "west",
                5 => "east",
                _ => facing,
            };
            state = with(
                named(&block)?.default,
                &[
                    ("rotation", &rotation),
                    ("facing", facing),
                    ("attached", "false"),
                    ("waterlogged", if water { "true" } else { "false" }),
                ],
            )?;
        }
        _ => {}
    }
    Some(vec![(pos, state)])
}
