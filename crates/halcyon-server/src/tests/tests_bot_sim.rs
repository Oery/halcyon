use super::*;
use crate::bot::brain::Mode;
use crate::bot::nav::Budget;

use crate::bot::test_map as map;
use map::MapView;

/// A miniature Parkour Spiral: a 2x2 spire with single-block steps around it,
/// each one block higher and one block apart, starting from a small pad.
fn spiral() -> (MapView, Transform, [i32; 3]) {
    let mut view = MapView::default();
    for y in 0..=30 {
        for [x, z] in [[-1, -1], [-1, 0], [0, -1], [0, 0]] {
            view.set([x, y, z], map::stone());
        }
    }
    view.floor([2, 5], 10, [-6, -4]);
    let ring = [[3, -2], [3, 0], [3, 2], [1, 3], [-1, 3], [-3, 2], [-3, 0], [-3, -2], [-1, -3]];
    let mut top = 10;
    for [x, z] in ring {
        top += 1;
        view.pillar(x, top, z);
    }
    let spawn = Transform { x: 4.5, y: 11.0, z: -5.5, on_ground: true, ..Transform::default() };
    (view, spawn, [-1, top + 1, -3])
}

fn climb(level: u8, seed: u64, limit: u32) -> (SimBot, u32) {
    let (view, spawn, goal) = spiral();
    let mut nav = NavService::new(Budget::Unlimited);
    nav.goal = Some(goal);
    let mut bot = SimBot::new(BotProfile::new(level), seed, spawn);
    for tick in 0..limit {
        bot.step(&mut nav, &view);
        if bot.brain.mode() == Mode::Finished {
            return (bot, tick);
        }
    }
    panic!(
        "level {level} bot did not finish: mode {:?} at {:?}, max y {}, deaths {}",
        bot.brain.mode(),
        bot.position(),
        bot.max_y,
        bot.deaths
    );
}

#[test]
fn a_strong_bot_climbs_the_spiral() {
    let (bot, ticks) = climb(100, 1, 4_000);
    eprintln!("level 100: {ticks} ticks, {:?}, deaths {}", bot.brain.stats, bot.deaths);
    assert!(bot.position().y >= 20.0);
}

#[test]
fn weak_bots_climb_too_with_mistakes_and_recoveries() {
    let mut falls = 0;
    for seed in 1..=4 {
        let (bot, ticks) = climb(0, seed, 20_000);
        eprintln!(
            "level 0 seed {seed}: {ticks} ticks, {:?}, deaths {}",
            bot.brain.stats, bot.deaths
        );
        falls += bot.brain.stats.falls;
    }
    eprintln!("level 0 falls over four climbs: {falls}");
}

#[test]
fn a_bot_that_falls_in_water_swims_ashore_and_carries_on() {
    let water = map::id("minecraft:water[level=0]");
    let mut view = MapView::default();
    view.floor([-6, 6], 10, [-6, 6]);
    for x in -3..=3 {
        for z in -3..=3 {
            view.set([x, 8, z], map::stone()).set([x, 9, z], water).set([x, 10, z], water);
        }
    }
    let spawn = Transform { x: 0.5, y: 10.2, z: 0.5, ..Transform::default() };
    let mut nav = NavService::new(Budget::Unlimited);
    nav.goal = Some([5, 11, 5]);
    let mut bot = SimBot::new(BotProfile::new(50), 3, spawn);
    let mut swam = false;
    for _ in 0..2_000 {
        bot.step(&mut nav, &view);
        swam |= bot.brain.mode() == Mode::Swimming;
        if bot.brain.mode() == Mode::Finished {
            break;
        }
    }
    assert!(swam);
    assert_eq!(bot.brain.mode(), Mode::Finished, "ended at {:?}", bot.position());
}

#[test]
fn an_off_centre_start_steps_onto_its_cell_first() {
    let (view, _, goal) = spiral();
    // The pad's corner, well off any cell centre.
    let spawn = Transform { x: 2.08, y: 11.0, z: -4.07, on_ground: true, ..Transform::default() };
    let mut nav = NavService::new(Budget::Unlimited);
    nav.goal = Some(goal);
    let mut bot = SimBot::new(BotProfile::new(80), 5, spawn);
    for _ in 0..6_000 {
        bot.step(&mut nav, &view);
        if bot.brain.mode() == Mode::Finished {
            return;
        }
    }
    panic!("stuck at {:?} in {:?}", bot.position(), bot.brain.mode());
}

#[test]
fn bots_pass_gold_plate_checkpoints_in_order() {
    let (mut view, spawn, goal) = spiral();
    // The fifth step, [-1, 15, 3] top, carries a gold pressure plate.
    view.set([-1, 16, 3], map::id("minecraft:light_weighted_pressure_plate[power=0]"));
    let mut nav = NavService::new(Budget::Unlimited);
    nav.checkpoints = nav.detect_checkpoints(&view, [0, 11, 0], 10);
    assert_eq!(nav.checkpoints, vec![[-1, 16, 3]]);
    nav.goal = Some(goal);
    let mut bot = SimBot::new(BotProfile::new(90), 2, spawn);
    let mut passed_at = None;
    for tick in 0..6_000 {
        bot.step(&mut nav, &view);
        if bot.brain.checkpoint() == 1 && passed_at.is_none() {
            passed_at = Some(tick);
        }
        if bot.brain.mode() == Mode::Finished {
            break;
        }
    }
    assert_eq!(bot.brain.mode(), Mode::Finished);
    assert!(passed_at.is_some());
}

#[test]
fn bots_open_closed_doors_on_their_route() {
    let door = |half: &str| {
        map::id(&format!(
            "minecraft:oak_door[facing=east,half={half},hinge=left,open=false,powered=false]"
        ))
    };
    let mut view = MapView::default();
    view.floor([-3, 8], 10, [-3, 3]);
    // A wall across the floor, passable only through the door.
    for z in -3..=3 {
        for y in 11..=13 {
            view.set([3, y, z], map::stone());
        }
    }
    view.set([3, 11, 0], door("lower")).set([3, 12, 0], door("upper"));
    let spawn = Transform { x: 0.5, y: 11.0, z: 0.5, on_ground: true, ..Transform::default() };
    let mut nav = NavService::new(Budget::Unlimited);
    nav.goal = Some([6, 11, 0]);
    let mut bot = SimBot::new(BotProfile::new(70), 4, spawn);
    let mut uses = 0;
    for _ in 0..3_000 {
        bot.step(&mut nav, &view);
        if let Some(pos) = bot.brain.take_use() {
            // What the lobby owner does with the bot's hand use.
            let cells =
                crate::interaction::toggled_cells(&view, pos, bot.position().yaw).expect("a door");
            for (cell, state) in cells {
                view.set(cell, state);
            }
            uses += 1;
        }
        if bot.brain.mode() == Mode::Finished {
            break;
        }
    }
    assert_eq!(bot.brain.mode(), Mode::Finished, "stuck at {:?}", bot.position());
    assert_eq!(uses, 1, "opened the door once");
}

#[test]
fn runs_are_reproducible() {
    let (a, ta) = climb(50, 9, 10_000);
    let (b, tb) = climb(50, 9, 10_000);
    assert_eq!(ta, tb);
    assert_eq!(a.position(), b.position());
}
