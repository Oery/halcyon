use super::*;
use crate::blockstates::block_state_id;

fn info(key: &str) -> &'static StateInfo {
    state_info(block_state_id(key).unwrap() as BlockStateId)
}

#[test]
fn catalog_matches_versioned_registry() {
    assert_eq!(data::STATES.len(), crate::blockstates::BLOCK_STATES.len());
    assert_eq!(fluid_data::FLUIDS.len(), data::STATES.len());
    assert_eq!(size_of::<StateInfo>(), 4);
    for &(_, id) in crate::blockstates::BLOCK_STATES {
        assert!(try_state_info(id.try_into().unwrap()).is_some());
        let fluid = fluid_info(id as u16);
        assert_eq!(fluid.kind != FluidKind::Empty, state_info(id as u16).flags.has_fluid());
        assert!(fluid.amount <= 8);
        assert!(!fluid.supported || fluid.kind == FluidKind::Water);
    }
    for state in data::STATES {
        for shape in state.collision_boxes() {
            for axis in 0..3 {
                assert!(shape.min[axis].is_finite() && shape.max[axis].is_finite());
                assert!(shape.min[axis] < shape.max[axis]);
            }
        }
    }
    assert!(try_state_info(u16::MAX).is_none());
}

#[test]
fn static_shapes_and_unsupported_motion_remain_separate() {
    assert!(info("minecraft:air").collision_boxes().is_empty());
    assert_eq!(info("minecraft:stone").kind, BlockKind::Full);
    let slime = info("minecraft:slime_block");
    assert!(slime.uncertain_motion());
    assert!(!slime.contextual_geometry());
    assert_eq!(slime.collision_boxes(), info("minecraft:stone").collision_boxes());
    let water = info("minecraft:water[level=0]");
    assert!(water.flags.has_fluid() && water.uncertain_motion());
    assert!(water.collision_boxes().is_empty());
    let snow = info("minecraft:snow[layers=1]");
    assert!(snow.collision_boxes().is_empty());
    assert_eq!(info("minecraft:snow[layers=8]").collision_boxes()[0].max[1], 0.875);
    assert!(info("minecraft:powder_snow").contextual_geometry());
}

#[test]
fn ground_profiles_match_target_surface_properties_and_keep_media_disabled() {
    let motion = |key| motion_info(block_state_id(key).unwrap() as BlockStateId);
    assert_eq!(motion("minecraft:ice").friction, 0.98);
    assert_eq!(motion("minecraft:blue_ice").friction, 0.989);
    assert_eq!(motion("minecraft:soul_sand").speed, 0.4);
    assert_eq!(motion("minecraft:honey_block").jump, 0.5);
    assert_eq!(motion("minecraft:slime_block").bounce, 1.0);
    for key in ["minecraft:sand", "minecraft:red_sand", "minecraft:gravel"] {
        let properties = block_properties(block_state_id(key).unwrap() as u16);
        assert!(properties.falls() && properties.suffocating());
        assert!(!properties.free() && !properties.replaceable());
    }
    assert!(block_properties(1).suffocating());
    assert!(!block_properties(block_state_id("minecraft:glass").unwrap() as u16).suffocating());
    assert_eq!(motion("minecraft:shelf_mushroom[age=0,facing=north]").bounce, 0.75);
    assert_eq!(
        motion("minecraft:small_dripleaf[facing=west,half=lower,waterlogged=false]"),
        MotionInfo::default()
    );
    assert_eq!(
        motion("minecraft:small_dripleaf[facing=west,half=lower,waterlogged=true]").surface,
        Surface::Ordinary
    );
    for key in [
        "minecraft:ice",
        "minecraft:soul_sand",
        "minecraft:slime_block",
        "minecraft:honey_block",
        "minecraft:mud",
    ] {
        assert_ne!(motion(key).surface, Surface::Unsupported, "{key}");
    }
    for key in [
        "minecraft:cobweb",
        "minecraft:powder_snow",
        "minecraft:ladder[facing=north,waterlogged=false]",
    ] {
        assert_eq!(motion(key).surface, Surface::Unsupported, "{key}");
    }
}

#[test]
fn slabs_stairs_trapdoors_and_tall_shapes_keep_exact_geometry() {
    for (slab_type, min, max) in [("bottom", 0.0, 0.5), ("top", 0.5, 1.0), ("double", 0.0, 1.0)] {
        let slab = info(&format!("minecraft:oak_slab[type={slab_type},waterlogged=false]"));
        assert_eq!(slab.kind, BlockKind::Slab);
        assert_eq!(slab.collision_boxes(), &[Aabb { min: [0.0, min, 0.0], max: [1.0, max, 1.0] }]);
    }
    for (facing, min, max) in [
        ("north", [0.0, 0.0, 0.8125], [1.0; 3]),
        ("south", [0.0; 3], [1.0, 1.0, 0.1875]),
        ("east", [0.0; 3], [0.1875, 1.0, 1.0]),
        ("west", [0.8125, 0.0, 0.0], [1.0; 3]),
    ] {
        for (half, height) in [("bottom", 0.0), ("top", 0.8125)] {
            let open = info(&format!(
                "minecraft:spruce_trapdoor[facing={facing},half={half},open=true,powered=false,waterlogged=false]"
            ));
            assert_eq!(open.collision_boxes(), &[Aabb { min, max }]);
            let closed = info(&format!(
                "minecraft:spruce_trapdoor[facing={facing},half={half},open=false,powered=false,waterlogged=false]"
            ));
            assert_eq!(
                closed.collision_boxes(),
                &[Aabb { min: [0.0, height, 0.0], max: [1.0, height + 0.1875, 1.0] }]
            );
        }
    }
    for facing in ["north", "south", "east", "west"] {
        for half in ["top", "bottom"] {
            for shape in ["straight", "inner_left", "inner_right", "outer_left", "outer_right"] {
                let stair = info(&format!(
                    "minecraft:oak_stairs[facing={facing},half={half},shape={shape},waterlogged=false]"
                ));
                assert_eq!(stair.kind, BlockKind::Stairs);
                assert!(!stair.contextual_geometry());
                assert!(stair.collision_boxes().len() >= 2);
                let wet = info(&format!(
                    "minecraft:oak_stairs[facing={facing},half={half},shape={shape},waterlogged=true]"
                ));
                assert!(wet.uncertain_motion());
                assert_eq!(stair.collision_boxes(), wet.collision_boxes());
            }
        }
    }
    let fence = info(
        "minecraft:oak_fence[east=false,north=false,south=false,waterlogged=false,west=false]",
    );
    assert!(fence.collision_boxes().iter().any(|shape| shape.max[1] == 1.5));
}

#[test]
fn sections_retain_y_z_x_order_without_uniform_allocations() {
    assert_eq!(Section::Uniform(1).backing_bytes(), 0);
    let mut blocks = Box::new([0; 4096]);
    blocks[(3 << 8) | (5 << 4) | 7] = 42;
    let section = Section::from_states(blocks);
    assert_eq!(section.block_at(7, 3, 5), 42);
    assert_eq!(section.block_at(3, 7, 5), 0);
    assert_eq!(section.backing_bytes(), 8192);
    assert!(matches!(Section::from_states(Box::new([1; 4096])), Section::Uniform(1)));
}
