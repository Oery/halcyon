use super::*;

fn id(key: &str) -> BlockStateId {
    block_state_id(key).unwrap() as BlockStateId
}

#[test]
fn doors_toggle_both_halves() {
    let lower =
        id("minecraft:oak_door[facing=north,half=lower,hinge=left,open=false,powered=false]");
    let open_lower =
        id("minecraft:oak_door[facing=north,half=lower,hinge=left,open=true,powered=false]");
    let open_upper =
        id("minecraft:oak_door[facing=north,half=upper,hinge=left,open=true,powered=false]");
    assert_eq!(
        opening(lower),
        Some(Opening::Door { toggled: open_lower, paired: open_upper, upper: false })
    );
    let Some(Opening::Door { toggled, upper: true, .. }) = opening(open_upper) else {
        panic!("upper half must be usable");
    };
    assert_eq!(
        toggled,
        id("minecraft:oak_door[facing=north,half=upper,hinge=left,open=false,powered=false]")
    );
}

#[test]
fn redstone_only_openings_are_unavailable() {
    assert_eq!(
        opening(id(
            "minecraft:iron_door[facing=north,half=lower,hinge=left,open=false,powered=false]"
        )),
        None
    );
    let trapdoor = |open, waterlogged| {
        id(&format!(
            "minecraft:oak_trapdoor[facing=north,half=bottom,open={open},powered=false,waterlogged={waterlogged}]"
        ))
    };
    assert_eq!(
        opening(trapdoor(false, false)),
        Some(Opening::Trapdoor { toggled: trapdoor(true, false) })
    );
    // Water stays in place; fluid flow is not simulated.
    assert_eq!(
        opening(trapdoor(false, true)),
        Some(Opening::Trapdoor { toggled: trapdoor(true, true) })
    );
    assert_eq!(opening(id("minecraft:stone")), None);
}

#[test]
fn closed_gates_can_swing_away_from_the_user() {
    let gate = |facing, open| {
        id(&format!(
            "minecraft:oak_fence_gate[facing={facing},in_wall=false,open={open},powered=false]"
        ))
    };
    assert_eq!(
        opening(gate("north", false)),
        Some(Opening::FenceGate {
            toggled: gate("north", true),
            open: false,
            facing: 0,
            reversed: gate("south", true),
        })
    );
}

#[test]
fn every_opening_result_is_a_supported_edit() {
    use crate::terrain::{Surface, motion_info, try_state_info};
    let mut count = 0;
    for &(_, id) in BLOCK_STATES {
        let targets = match opening(id as BlockStateId) {
            Some(Opening::Door { toggled, paired, .. }) => vec![toggled, paired],
            Some(Opening::Trapdoor { toggled }) => vec![toggled],
            Some(Opening::FenceGate { toggled, reversed, .. }) => vec![toggled, reversed],
            None => continue,
        };
        count += 1;
        for target in targets {
            assert!(try_state_info(target).is_some());
            assert_ne!(motion_info(target).surface, Surface::Unsupported, "state {target}");
        }
    }
    assert!(count > 1_000);
}
