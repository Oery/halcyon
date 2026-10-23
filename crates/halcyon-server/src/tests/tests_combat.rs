use super::*;

fn item(name: &str) -> ItemStack {
    let id = ITEMS.iter().position(|(item, ..)| *item == name).unwrap();
    ItemStack::plain(id as i32, 1).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn weapons_use_26_3_attributes_and_disable_unsupported_attacks() {
    let sword = weapon(&item("minecraft:diamond_sword")).unwrap();
    assert_eq!(sword.damage, 7.0);
    assert!(close(sword.speed, 1.6) && sword.sword);
    let axe = weapon(&item("minecraft:netherite_axe")).unwrap();
    assert_eq!((axe.damage, axe.sword), (10.0, false));
    assert!(close(axe.speed, 1.0));
    assert!(close(weapon(&item("minecraft:iron_axe")).unwrap().speed, 0.9));
    let hoe = weapon(&item("minecraft:iron_hoe")).unwrap();
    assert_eq!(hoe.damage, 1.0);
    assert!(close(hoe.speed, 3.0));
    assert_eq!(weapon(&item("minecraft:stone_shovel")).unwrap().damage, 3.5);
    assert_eq!(weapon(&item("minecraft:trident")).unwrap().damage, 9.0);
    assert_eq!(weapon(&item("minecraft:stick")), Some(Weapon::FIST));
    assert_eq!(weapon(&ItemStack::default()), Some(Weapon::FIST));
    assert_eq!(weapon(&item("minecraft:mace")), None);
    assert_eq!(weapon(&item("minecraft:iron_spear")), None);
}

#[test]
fn attack_strength_charges_with_the_weapon_speed() {
    assert_eq!(Weapon::FIST.strength(0), 0.1);
    assert!(!strong(Weapon::FIST.strength(4)));
    assert!(strong(Weapon::FIST.strength(5)));
    let sword = weapon(&item("minecraft:iron_sword")).unwrap();
    assert!(!strong(sword.strength(10)));
    // The float modifier makes the speed 1.5999999, exactly as in vanilla.
    assert!(strong(sword.strength(12)) && sword.strength(12) < 1.0);
    assert_eq!(sword.strength(13), 1.0);
    assert_eq!(scaled_damage(sword, 1.0), 6.0);
    assert!((scaled_damage(sword, 0.5) - 2.4).abs() < 1e-5);
    assert!((scaled_damage(sword, 0.0) - 1.2).abs() < 1e-5);
}

#[test]
fn armor_absorbs_damage_by_points_and_toughness() {
    let diamond = [
        ItemStack::default(),
        ItemStack::default(),
        item("minecraft:diamond_boots"),
        item("minecraft:diamond_leggings"),
        item("minecraft:diamond_chestplate"),
        item("minecraft:diamond_helmet"),
    ];
    let armor = Armor::worn(&diamond);
    assert_eq!((armor.points, armor.toughness), (20.0, 8.0));
    assert!((armor.absorb(7.0) - 1.89).abs() < 1e-5);
    let leather = Armor { points: 7.0, ..Armor::default() };
    assert!((leather.absorb(10.0) - 9.2).abs() < 1e-5);
    // Points never fall below a fifth of the armor value.
    assert!((leather.absorb(100.0) - 100.0 * (1.0 - 1.4 / 25.0)).abs() < 1e-3);

    let mut misplaced = diamond.clone();
    misplaced[2] = item("minecraft:diamond_helmet");
    assert_eq!(Armor::worn(&misplaced).points, 17.0);
    let netherite = [
        ItemStack::default(),
        ItemStack::default(),
        item("minecraft:netherite_boots"),
        item("minecraft:netherite_leggings"),
        item("minecraft:netherite_chestplate"),
        item("minecraft:netherite_helmet"),
    ];
    let armor = Armor::worn(&netherite);
    assert_eq!(armor.toughness, 12.0);
    assert!((armor.knockback_resistance - 0.4).abs() < 1e-6);
}

#[test]
fn knockback_halves_motion_and_pushes_away() {
    let never = || unreachable!();
    let pushed = knockback([0.2, -0.1, 0.0], true, 0.4, 0.0, [3.0, 0.0], never).unwrap();
    assert!(close(pushed[0], 0.1 - 0.4) && close(pushed[1], 0.35) && close(pushed[2], 0.0));
    let airborne = knockback([0.0, -0.3, 0.0], false, 0.4, 0.0, [0.0, -2.0], never).unwrap();
    assert!(close(airborne[1], -0.3) && close(airborne[2], 0.4));
    let resisted = knockback([0.0; 3], true, 0.4, 0.5, [1.0, 0.0], never).unwrap();
    assert!(close(resisted[0], -0.2) && close(resisted[1], 0.2));
    assert_eq!(knockback([0.0; 3], true, 0.4, 1.0, [1.0, 0.0], never), None);
    // A degenerate direction is randomized, keeping the full strength.
    let mut values = [0.9, 0.1, 0.2, 0.6].into_iter().cycle();
    let random = knockback([0.0; 3], true, 0.4, 0.0, [0.0, 0.0], || values.next().unwrap());
    let [x, _, z] = random.unwrap();
    assert!(close((x * x + z * z).sqrt(), 0.4));
}

#[test]
fn sweeps_need_a_grounded_slow_sword_and_reach_needs_a_close_box() {
    let sword = weapon(&item("minecraft:stone_sword")).unwrap();
    assert!(sweeps(sword, true, [0.2, 0.5, 0.1]));
    assert!(!sweeps(sword, false, [0.0; 3]));
    assert!(!sweeps(sword, true, [0.2, 0.0, 0.2]));
    assert!(!sweeps(Weapon::FIST, true, [0.0; 3]));

    let target = |x: f64| Aabb { min: [x, 0.0, -0.3], max: [x + 0.6, 1.8, 0.3] };
    assert!(in_reach([0.0, 1.62, 0.0], target(5.9), false));
    assert!(!in_reach([0.0, 1.62, 0.0], target(6.1), false));
    assert!(in_reach([0.0, 1.62, 0.0], target(7.9), true));
    assert!(close(distance_sqr(target(1.0), [0.0, 3.8, 0.0]), 5.0));
}
