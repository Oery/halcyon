//! Ground surface properties extracted from the official Java 26.3 client.
//! Client SHA-256: 4508d006323f24fa02876310c192d739af56516eb259000ac50f0909a68c9a2d.
//! Enumerate Block.BLOCK_STATE_REGISTRY after Bootstrap.bootStrap(); read
//! getFriction/getSpeedFactor/getJumpFactor/getBounceRestitution/getFallDistanceReduction.
//! Ranges retain the target registry's block-state IDs. Unsupported media remain disabled.
use super::{MotionInfo, Surface};
pub(super) fn special_motion(id: u16) -> Option<MotionInfo> {
    Some(match id {
        2030..=2045 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:white_bed
        2046..=2061 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:orange_bed
        2062..=2077 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:magenta_bed
        2078..=2093 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:light_blue_bed
        2094..=2109 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:yellow_bed
        2110..=2125 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:lime_bed
        2126..=2141 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:pink_bed
        2142..=2157 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:gray_bed
        2158..=2173 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:light_gray_bed
        2174..=2189 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:cyan_bed
        2190..=2205 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:purple_bed
        2206..=2221 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:blue_bed
        2222..=2237 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:brown_bed
        2238..=2253 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:green_bed
        2254..=2269 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:red_bed
        2270..=2285 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:black_bed
        2286..=2301 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:straw_bed
        8597 => MotionInfo {
            friction: 0.98,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:ice
        8668 => MotionInfo {
            friction: 0.6,
            speed: 0.4,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:soul_sand
        11227..=11234 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.75,
            fall_reduction: 0.5,
            surface: Surface::Ordinary,
        }, // minecraft:shelf_mushroom
        14379 => MotionInfo {
            friction: 0.8,
            speed: 1.0,
            jump: 1.0,
            bounce: 1.0,
            fall_reduction: 0.0,
            surface: Surface::Slime,
        }, // minecraft:slime_block
        14740..=14742 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Hay,
        }, // minecraft:hay_block
        14761 => MotionInfo {
            friction: 0.98,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:packed_ice
        16822..=16825 => MotionInfo {
            friction: 0.98,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:frosted_ice
        18632 => MotionInfo {
            friction: 0.989,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:blue_ice
        25173 => MotionInfo {
            friction: 0.6,
            speed: 0.4,
            jump: 0.5,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Honey,
        }, // minecraft:honey_block
        33753..=33768 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:small_dripleaf
        33772 => MotionInfo {
            friction: 0.6,
            speed: 1.0,
            jump: 1.0,
            bounce: 0.0,
            fall_reduction: 0.0,
            surface: Surface::Ordinary,
        }, // minecraft:mud
        _ => return None,
    })
}
