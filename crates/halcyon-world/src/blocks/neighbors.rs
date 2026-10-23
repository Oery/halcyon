//! Bounded shape/support propagation for the explicitly supported block families.
use super::*;

pub const MAX_EDIT_CELLS: usize = 64;
const MAX_CHECKS: usize = 512;

pub fn updates(
    view: &impl BlockView,
    initial: &[([i32; 3], BlockStateId)],
) -> Option<Vec<([i32; 3], BlockStateId)>> {
    let mut cells = Vec::new();
    let mut pending = Vec::new();
    for &(pos, state) in initial {
        if let Some((_, old)) = cells.iter_mut().find(|(p, _)| *p == pos) {
            *old = state;
        } else {
            cells.push((pos, state));
        }
        pending.push(pos);
        pending.extend((0..6).map(|d| offset(pos, d)));
    }
    let mut checks = 0;
    while let Some(pos) = pending.pop() {
        checks += 1;
        if checks > MAX_CHECKS || cells.len() > MAX_EDIT_CELLS {
            return None;
        }
        let current = PlannedView { base: view, cells: &cells };
        let Some(state) = current.block_at(pos[0], pos[1], pos[2]) else {
            continue;
        };
        let updated = neighbor_state(&current, pos, state)?;
        if updated == state {
            continue;
        }
        if let Some((_, old)) = cells.iter_mut().find(|(p, _)| *p == pos) {
            *old = updated;
        } else {
            cells.push((pos, updated));
        }
        pending.extend((0..6).map(|d| offset(pos, d)));
    }
    if cells.len() > MAX_EDIT_CELLS {
        return None;
    }
    Some(cells)
}

fn at(view: &impl BlockView, pos: [i32; 3], direction: usize) -> Option<BlockStateId> {
    let p = offset(pos, direction);
    view.block_at(p[0], p[1], p[2])
}

fn supports(view: &impl BlockView, pos: [i32; 3], direction: usize) -> bool {
    at(view, pos, direction).is_some_and(|s| sturdy(s, direction ^ 1))
}

pub fn neighbor_state(
    view: &impl BlockView,
    pos: [i32; 3],
    state: BlockStateId,
) -> Option<BlockStateId> {
    let Some(kind) = placement(state) else {
        return Some(state);
    };
    match kind {
        Placement::Door => {
            let upper = property(state, "half") == Some("upper");
            let other = at(view, pos, if upper { 0 } else { 1 });
            if !other.is_some_and(|s| {
                metadata(s).name == metadata(state).name
                    && property(s, "half") != property(state, "half")
            }) || (!upper && !supports(view, pos, 0))
            {
                return Some(replacement(state));
            }
            if upper {
                let lower = other?;
                return with(lower, &[("half", "upper")]);
            }
        }
        Placement::Sign => {
            let name = metadata(state).name;
            let supported = if name.contains("_wall_hanging_sign") {
                let facing = horizontal(property(state, "facing")?);
                [2, 3, 4, 5].into_iter().filter(|d| (*d / 2) != (facing / 2)).any(|d| {
                    supports(view, pos, d)
                        || at(view, pos, d).is_some_and(|s| {
                            metadata(s).name.ends_with("_wall_hanging_sign")
                                && property(s, "facing") == property(state, "facing")
                        })
                })
            } else if name.ends_with("_hanging_sign") {
                at(view, pos, 1).is_some_and(center_down)
            } else if name.contains("_wall_sign") {
                at(view, pos, horizontal(property(state, "facing")?) ^ 1).is_some_and(solid)
            } else {
                at(view, pos, 0).is_some_and(solid)
            };
            if !supported {
                return Some(replacement(state));
            }
        }
        Placement::Stairs => return stair_shape(view, pos, state),
        Placement::Fence | Placement::Wall => {
            let mut result = state;
            let mut connected = [false; 4];
            for (i, name) in HORIZONTAL.iter().enumerate() {
                let direction = horizontal(name);
                connected[i] =
                    at(view, pos, direction).is_some_and(|other| connects(state, other, direction));
                let value = if kind == Placement::Wall {
                    if !connected[i] {
                        "none"
                    } else if at(view, pos, 1).is_some_and(|s| wall_cover(s, i)) {
                        "tall"
                    } else {
                        "low"
                    }
                } else if connected[i] {
                    "true"
                } else {
                    "false"
                };
                result = with(result, &[(name, value)])?;
            }
            if kind == Placement::Wall {
                let straight = connected == [true, false, true, false]
                    || connected == [false, true, false, true];
                let above = at(view, pos, 1);
                let tall_straight = straight
                    && HORIZONTAL
                        .iter()
                        .all(|name| matches!(property(result, name), Some("none" | "tall")));
                let post = !straight
                    || (!tall_straight
                        && above.is_some_and(|s| {
                            property(s, "up") == Some("true") || wall_cover(s, 4)
                        }));
                result = with(result, &[("up", if post { "true" } else { "false" })])?;
            }
            return Some(result);
        }
        Placement::Gate => {
            let facing = horizontal(property(state, "facing")?);
            let side = if facing < 4 { 4 } else { 2 };
            let wall = [side, side ^ 1]
                .into_iter()
                .any(|d| at(view, pos, d).is_some_and(|s| placement(s) == Some(Placement::Wall)));
            return with(state, &[("in_wall", if wall { "true" } else { "false" })]);
        }
        _ => {}
    }
    Some(state)
}

fn connects(state: BlockStateId, other: BlockStateId, direction: usize) -> bool {
    let wall = placement(state) == Some(Placement::Wall);
    let kind = placement(other);
    (kind == Some(Placement::Wall) && wall)
        || (kind == Some(Placement::Fence)
            && !wall
            && (metadata(state).name == "minecraft:nether_brick_fence")
                == (metadata(other).name == "minecraft:nether_brick_fence"))
        || (kind == Some(Placement::Gate)
            && horizontal(property(other, "facing").unwrap_or("north")) / 2 != direction / 2)
        || (!connection_exception(other) && sturdy(other, direction ^ 1))
}

fn stair_shape(view: &impl BlockView, pos: [i32; 3], state: BlockStateId) -> Option<BlockStateId> {
    let facing = property(state, "facing")?;
    let direction = horizontal(facing);
    let same = |s| {
        placement(s) == Some(Placement::Stairs) && property(s, "half") == property(state, "half")
    };
    let different = |direction| {
        !at(view, pos, direction).is_some_and(|s| same(s) && property(s, "facing") == Some(facing))
    };
    let index = HORIZONTAL.iter().position(|s| *s == facing)?;
    let left = HORIZONTAL[(index + 3) % 4];
    if let Some(front) = at(view, pos, direction).filter(|s| same(*s)) {
        let other = property(front, "facing")?;
        if horizontal(other) / 2 != direction / 2 && different(horizontal(other) ^ 1) {
            return with(
                state,
                &[("shape", if other == left { "outer_left" } else { "outer_right" })],
            );
        }
    }
    if let Some(back) = at(view, pos, direction ^ 1).filter(|s| same(*s)) {
        let other = property(back, "facing")?;
        if horizontal(other) / 2 != direction / 2 && different(horizontal(other)) {
            return with(
                state,
                &[("shape", if other == left { "inner_left" } else { "inner_right" })],
            );
        }
    }
    with(state, &[("shape", "straight")])
}
