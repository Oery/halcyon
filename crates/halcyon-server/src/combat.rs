//! Vanilla 26.3 melee rules: weapons, attack strength, criticals, sweeps,
//! knockback and armor. Pure calculations; the lobby owner validates targets
//! and applies the results.

use halcyon_world::Aabb;
use mc_protocol::data::items::ITEMS;
use mc_protocol::types::ItemStack;

/// Server allowance over the entity interaction range when accepting an attack.
const REACH_BUFFER: f64 = 3.0;
/// Base `movement_speed`; a sweep requires moving slower than 2.5 times it.
const MOVEMENT_SPEED: f64 = 0.1f32 as f64;
pub const SWEEP_RANGE_SQR: f64 = 9.0;
/// Every fresh hit pushes the victim away from its attacker.
pub const HURT_KNOCKBACK: f64 = 0.4f32 as f64;
/// Sweeps and sprint attacks push along the attacker's yaw.
pub const SWEEP_KNOCKBACK: f64 = 0.4f32 as f64;
pub const SPRINT_KNOCKBACK: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weapon {
    /// The `attack_damage` attribute, including the player's base of 1.
    pub damage: f32,
    /// The `attack_speed` attribute, including the player's base of 4.
    pub speed: f64,
    pub sword: bool,
}

impl Weapon {
    const FIST: Self = Self { damage: 1.0, speed: 4.0, sword: false };

    /// Item attribute modifiers are added as floats, then to the base as doubles.
    fn tool(damage: f32, speed: f32, sword: bool) -> Self {
        Self { damage: (1.0 + f64::from(damage)) as f32, speed: 4.0 + f64::from(speed), sword }
    }

    /// Ticks for a full charge (`getCurrentItemAttackStrengthDelay`).
    pub fn delay(self) -> f32 {
        (1.0 / self.speed * 20.0) as f32
    }

    /// `getAttackStrengthScale(0.5)` after `ticks` since the last reset.
    pub fn strength(self, ticks: u32) -> f32 {
        ((ticks as f32 + 0.5) / self.delay()).clamp(0.0, 1.0)
    }
}

/// None when the held item's attack depends on unsupported mechanics: mace
/// smashes and spear charges are disabled rather than approximated.
pub fn weapon(item: &ItemStack) -> Option<Weapon> {
    if item.is_empty() {
        return Some(Weapon::FIST);
    }
    let name = ITEMS[item.item_id() as usize].0;
    weapon_named(name.strip_prefix("minecraft:").unwrap_or(name))
}

fn weapon_named(name: &str) -> Option<Weapon> {
    if name == "mace" || name.ends_with("_spear") {
        return None;
    }
    if name == "trident" {
        return Some(Weapon::tool(8.0, -2.9, false));
    }
    let Some((material, kind)) = name.rsplit_once('_') else {
        return Some(Weapon::FIST);
    };
    // ToolMaterial attack damage bonuses.
    let bonus = match material {
        "wooden" | "golden" => 0.0,
        "stone" | "copper" => 1.0,
        "iron" => 2.0,
        "diamond" => 3.0,
        "netherite" => 4.0,
        _ => return Some(Weapon::FIST),
    };
    // Items' per-tool damage and speed modifiers.
    let (damage, speed) = match (kind, material) {
        ("sword", _) => (3.0, -2.4),
        ("shovel", _) => (1.5, -3.0),
        ("pickaxe", _) => (1.0, -2.8),
        ("axe", "wooden") => (6.0, -3.2),
        ("axe", "stone" | "copper") => (7.0, -3.2),
        ("axe", "golden") => (6.0, -3.0),
        ("axe", "iron") => (6.0, -3.1),
        ("axe", _) => (5.0, -3.0),
        ("hoe", "wooden" | "golden") => (0.0, -3.0),
        ("hoe", "stone" | "copper") => (-1.0, -2.0),
        ("hoe", "iron") => (-2.0, -1.0),
        ("hoe", "diamond") => (-3.0, 0.0),
        ("hoe", _) => (-4.0, 0.0),
        _ => return Some(Weapon::FIST),
    };
    Some(Weapon::tool(damage + bonus, speed, kind == "sword"))
}

/// `baseDamageScaleFactor`: weak swings still deal a fifth of the damage.
pub fn scaled_damage(weapon: Weapon, strength: f32) -> f32 {
    weapon.damage * (0.2 + strength * strength * 0.8)
}

/// Strong attacks can critically hit, sweep and knock back while sprinting.
pub fn strong(strength: f32) -> bool {
    strength > 0.9
}

/// `isSweepAttack` once crit and sprint-knockback are excluded.
pub fn sweeps(weapon: Weapon, on_ground: bool, known_movement: [f64; 3]) -> bool {
    let [x, _, z] = known_movement;
    let limit = MOVEMENT_SPEED * 2.5;
    weapon.sword && on_ground && x * x + z * z < limit * limit
}

/// `AttackRange.isInRange` for the default range: eye-to-box distance within
/// the entity interaction range plus the server's allowance.
pub fn in_reach(eye: [f64; 3], target: Aabb, creative: bool) -> bool {
    let range = if creative { 5.0 } else { 3.0 } + REACH_BUFFER;
    distance_sqr(target, eye) <= range * range
}

pub fn intersects(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|axis| a.min[axis] < b.max[axis] && a.max[axis] > b.min[axis])
}

pub fn distance_sqr(bounds: Aabb, point: [f64; 3]) -> f64 {
    (0..3)
        .map(|axis| {
            let gap = (bounds.min[axis] - point[axis]).max(point[axis] - bounds.max[axis]).max(0.0);
            gap * gap
        })
        .sum()
}

/// `LivingEntity.knockback`: halve the current motion and push away from
/// `(x, z)`, lifting a grounded victim. A degenerate direction is replaced by
/// a tiny random one, as vanilla does.
pub fn knockback(
    velocity: [f64; 3],
    on_ground: bool,
    strength: f64,
    resistance: f64,
    [mut x, mut z]: [f64; 2],
    mut random: impl FnMut() -> f64,
) -> Option<[f64; 3]> {
    let strength = strength * (1.0 - resistance);
    if strength <= 0.0 {
        return None;
    }
    while x * x + z * z < 1.0e-5 {
        x = (random() - random()) * 0.01;
        z = (random() - random()) * 0.01;
    }
    let length = (x * x + z * z).sqrt();
    let [push_x, push_z] = [x / length * strength, z / length * strength];
    Some([
        velocity[0] / 2.0 - push_x,
        if on_ground {
            (velocity[1] / 2.0 + strength).min(0.4)
        } else {
            velocity[1]
        },
        velocity[2] / 2.0 - push_z,
    ])
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Armor {
    pub points: f32,
    pub toughness: f32,
    pub knockback_resistance: f64,
}

impl Armor {
    /// Worn armor from `[main, off, feet, legs, chest, head]` equipment.
    pub fn worn(equipment: &[ItemStack; 6]) -> Self {
        let mut armor = Self::default();
        for (slot, item) in equipment[2..].iter().enumerate() {
            if item.is_empty() {
                continue;
            }
            let name = ITEMS[item.item_id() as usize].0;
            if let Some(piece) = piece(name.strip_prefix("minecraft:").unwrap_or(name), slot) {
                armor.points += piece.points;
                armor.toughness += piece.toughness;
                armor.knockback_resistance += piece.knockback_resistance;
            }
        }
        armor.points = armor.points.min(30.0);
        armor.toughness = armor.toughness.min(20.0);
        armor.knockback_resistance = armor.knockback_resistance.min(1.0);
        armor
    }

    /// `CombatRules.getDamageAfterAbsorb` without enchantments.
    pub fn absorb(self, damage: f32) -> f32 {
        let toughness = 2.0 + self.toughness / 4.0;
        let effective = (self.points - damage / toughness).clamp(self.points * 0.2, 20.0);
        damage * (1.0 - effective / 25.0)
    }
}

/// ArmorMaterials defense in feet, legs, chest, head order.
fn piece(name: &str, slot: usize) -> Option<Armor> {
    let (material, kind) = name.rsplit_once('_')?;
    let expected = match kind {
        "boots" => 0,
        "leggings" => 1,
        "chestplate" => 2,
        "helmet" => 3,
        _ => return None,
    };
    if slot != expected {
        return None;
    }
    let (defense, toughness, knockback_resistance): ([f32; 4], f32, f64) = match material {
        "leather" => ([1.0, 2.0, 3.0, 1.0], 0.0, 0.0),
        "copper" => ([1.0, 3.0, 4.0, 2.0], 0.0, 0.0),
        "chainmail" => ([1.0, 4.0, 5.0, 2.0], 0.0, 0.0),
        "iron" | "turtle" => ([2.0, 5.0, 6.0, 2.0], 0.0, 0.0),
        "golden" => ([1.0, 3.0, 5.0, 2.0], 0.0, 0.0),
        "diamond" => ([3.0, 6.0, 8.0, 3.0], 2.0, 0.0),
        "netherite" => ([3.0, 6.0, 8.0, 3.0], 3.0, f64::from(0.1f32)),
        _ => return None,
    };
    Some(Armor { points: defense[slot], toughness, knockback_resistance })
}

#[cfg(test)]
#[path = "tests/tests_combat.rs"]
mod tests;
