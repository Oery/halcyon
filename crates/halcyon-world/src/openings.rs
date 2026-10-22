//! Hand-operated openings: doors, trapdoors and fence gates.
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::blockstates::{BLOCK_STATES, block_state_id};
use crate::terrain::BlockStateId;

const FACINGS: [&str; 4] = ["north", "east", "south", "west"];

/// The result of using an opening by hand, in horizontal facing order N, E, S, W.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// Vanilla copies the toggled half onto its partner through a shape update;
    /// `paired` is that partner state, `upper` locates the clicked half.
    Door {
        toggled: BlockStateId,
        paired: BlockStateId,
        upper: bool,
    },
    Trapdoor {
        toggled: BlockStateId,
    },
    /// A closed gate opened from its front swings away and takes the user's facing.
    FenceGate {
        toggled: BlockStateId,
        open: bool,
        facing: u8,
        reversed: BlockStateId,
    },
}

static OPENINGS: OnceLock<HashMap<BlockStateId, Opening>> = OnceLock::new();

/// Iron doors and trapdoors need redstone, which Halcyon excludes. Waterlogged
/// trapdoors keep their water; the flow vanilla schedules is not simulated yet.
pub fn opening(id: BlockStateId) -> Option<Opening> {
    OPENINGS
        .get_or_init(|| {
            let mut table = HashMap::new();
            for &(key, id) in BLOCK_STATES {
                let Some((name, _)) = key.split_once('[') else {
                    continue;
                };
                let opening = if name.ends_with("_door") && name != "minecraft:iron_door" {
                    door(key)
                } else if name.ends_with("_trapdoor") && name != "minecraft:iron_trapdoor" {
                    toggle(key).map(|toggled| Opening::Trapdoor { toggled })
                } else if name.ends_with("_fence_gate") {
                    gate(key)
                } else {
                    continue;
                };
                if let Some(opening) = opening {
                    table.insert(id as BlockStateId, opening);
                }
            }
            table
        })
        .get(&id)
        .copied()
}

fn door(key: &str) -> Option<Opening> {
    let upper = prop(key, "half")? == "upper";
    let open = if prop(key, "open")? == "true" { "false" } else { "true" };
    let paired = with(key, &[("open", open), ("half", if upper { "lower" } else { "upper" })])?;
    Some(Opening::Door { toggled: with(key, &[("open", open)])?, paired, upper })
}

fn gate(key: &str) -> Option<Opening> {
    let open = prop(key, "open")? == "true";
    let facing = prop(key, "facing")?;
    let facing = FACINGS.iter().position(|f| *f == facing)? as u8;
    let reversed =
        with(key, &[("open", "true"), ("facing", FACINGS[usize::from((facing + 2) % 4)])])?;
    Some(Opening::FenceGate { toggled: toggle(key)?, open, facing, reversed })
}

fn toggle(key: &str) -> Option<BlockStateId> {
    with(key, &[("open", if prop(key, "open")? == "true" { "false" } else { "true" })])
}

fn prop<'a>(key: &'a str, name: &str) -> Option<&'a str> {
    let props = key.split_once('[')?.1.strip_suffix(']')?;
    props.split(',').find_map(|p| p.strip_prefix(name)?.strip_prefix('='))
}

/// Registry keys are already canonical, so replacing values keeps them sorted.
fn with(key: &str, values: &[(&str, &str)]) -> Option<BlockStateId> {
    let (block, props) = key.split_once('[')?;
    let props = props.strip_suffix(']')?;
    let mut out = format!("{block}[");
    for (i, p) in props.split(',').enumerate() {
        if i > 0 {
            out.push(',');
        }
        let (k, v) = p.split_once('=')?;
        let v = values.iter().find(|(name, _)| *name == k).map_or(v, |(_, value)| value);
        out.push_str(k);
        out.push('=');
        out.push_str(v);
    }
    out.push(']');
    block_state_id(&out).map(|id| id as BlockStateId)
}

#[cfg(test)]
#[path = "tests/tests_openings.rs"]
mod tests;
