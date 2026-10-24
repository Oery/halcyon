//! Box duels: a row of glass boxes built in the sky over the map. Fighters
//! are paired into boxes in iron gear; a box is decided when one fighter
//! is left in it, and the round ends when every box is decided. Eliminated
//! players watch from a gallery alongside the row.
use std::collections::BTreeSet;

use halcyon_world::blockstates::block_state_id;
use halcyon_world::{BlockStateId, MAX_BLOCK_Y, World};
use mc_protocol::data::items::ITEMS;
use mc_protocol::types::ItemStack;

use super::GameEvent;
use super::matches::{Match, MatchRules, Outcome};
use crate::arena::BlockPos;
use crate::instance::{GameCtx, Transform};
use crate::inventory::{HotbarSlot, InventorySlot};
use crate::lifecycle::PlayerRules;

pub const MAP: &str = "Parkour Spiral";
pub const BOXES: usize = 4;
const CAPACITY: usize = 2 * BOXES;
/// Outer box size; the floor inside is two blocks narrower. Roomy enough
/// that knockback exchanges rarely end against the glass.
pub const BOX_SIZE: i32 = 19;
const BOX_GAP: i32 = 3;
const BOX_WALL: i32 = 5;
/// Outer gallery depth along z, next to the boxes' north walls.
const GALLERY_DEPTH: i32 = 7;
const GALLERY_WALL: i32 = 2;
/// Clearance between the highest map block under the arena and its floor.
const CLEARANCE: i32 = 6;

/// Where everything is, computed once from the map.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The floor block layer; players stand at `floor + 1`.
    pub floor: i32,
    /// North-west outer corner of each box.
    pub boxes: [[i32; 2]; BOXES],
    pub gallery: Transform,
    pub cells: Vec<(BlockPos, BlockStateId)>,
}

impl Layout {
    /// Centered on the map spawn above the terrain under it. None when part
    /// of the footprint is not loaded or there is no room below the build limit.
    pub fn new(world: &World) -> Option<Self> {
        let (sx, _, sz, _, _) = world.spawn;
        let width = BOXES as i32 * BOX_SIZE + (BOXES as i32 - 1) * BOX_GAP;
        let x0 = sx.floor() as i32 - width / 2;
        let z0 = sz.floor() as i32 - BOX_SIZE / 2;
        let gallery_z = z0 - GALLERY_DEPTH;
        let (x1, z1) = (x0 + width - 1, z0 + BOX_SIZE - 1);

        let mut highest = halcyon_world::MIN_BLOCK_Y;
        for x in x0..=x1 {
            for z in gallery_z..=z1 {
                world.block_at(x, 0, z)?;
                let top = (halcyon_world::MIN_BLOCK_Y..MAX_BLOCK_Y)
                    .rev()
                    .find(|&y| world.block_at(x, y, z).is_some_and(|state| state != 0));
                highest = highest.max(top.unwrap_or(highest));
            }
        }
        let floor = highest + CLEARANCE;
        if floor + BOX_WALL + 4 > MAX_BLOCK_Y {
            return None;
        }

        let floor_block = state("minecraft:smooth_stone")?;
        let glass = state("minecraft:glass")?;
        let mut cells = Vec::new();
        let boxes = std::array::from_fn(|i| [x0 + i as i32 * (BOX_SIZE + BOX_GAP), z0]);
        for [bx, bz] in boxes {
            enclosure(
                &mut cells,
                [bx, bz],
                [BOX_SIZE, BOX_SIZE],
                floor,
                BOX_WALL,
                floor_block,
                glass,
            );
        }
        enclosure(
            &mut cells,
            [x0, gallery_z],
            [width, GALLERY_DEPTH],
            floor,
            GALLERY_WALL,
            glass,
            glass,
        );
        let gallery = Transform {
            x: sx.floor() + 0.5,
            y: f64::from(floor + 1),
            z: f64::from(gallery_z + GALLERY_DEPTH / 2) + 0.5,
            // Facing south, into the boxes.
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        };
        Some(Self { floor, boxes, gallery, cells })
    }

    /// The `slot`-th fighter's start in box `index`, facing the box center.
    pub fn spawn(&self, index: usize, slot: usize) -> Transform {
        // Fighters start about eleven blocks apart across the middle.
        let (near, far) = (5.5, f64::from(BOX_SIZE) - 5.5);
        let corners = [[near, near], [far, far], [far, near]];
        let [bx, bz] = self.boxes[index].map(f64::from);
        let [cx, cz] = corners[slot % corners.len()];
        let center = f64::from(BOX_SIZE) / 2.0;
        let yaw = (-(center - cx)).atan2(center - cz).to_degrees() as f32;
        Transform {
            x: bx + cx,
            y: f64::from(self.floor + 1),
            z: bz + cz,
            yaw,
            pitch: 0.0,
            on_ground: false,
        }
    }
}

fn state(key: &str) -> Option<BlockStateId> {
    block_state_id(key).and_then(|id| BlockStateId::try_from(id).ok())
}

/// A floor of `size` at `y` with walls of `height` around its edge.
fn enclosure(
    cells: &mut Vec<(BlockPos, BlockStateId)>,
    [x0, z0]: [i32; 2],
    [dx, dz]: [i32; 2],
    y: i32,
    height: i32,
    floor: BlockStateId,
    wall: BlockStateId,
) {
    for x in x0..x0 + dx {
        for z in z0..z0 + dz {
            cells.push(([x, y, z], floor));
            let edge = x == x0 || z == z0 || x == x0 + dx - 1 || z == z0 + dz - 1;
            if edge {
                cells.extend((1..=height).map(|h| ([x, y + h, z], wall)));
            }
        }
    }
}

/// Box index for each roster slot: pairs in order, with an odd fighter
/// joining the last pair.
pub fn pairing(fighters: usize) -> Vec<usize> {
    let pairs = (fighters / 2).clamp(1, BOXES);
    (0..fighters).map(|slot| (slot / 2).min(pairs - 1)).collect()
}

/// What every fighter in a game wears and holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kit {
    /// An iron sword and full iron armor: about seven hits to win.
    Iron,
    /// A stone sword against full diamond armor: about sixteen hits, so
    /// duels last two to three times longer.
    Long,
}

impl Kit {
    fn items(self) -> [(InventorySlot, &'static str); 5] {
        let (sword, armor) = match self {
            Self::Iron => ("minecraft:iron_sword", "iron"),
            Self::Long => ("minecraft:stone_sword", "diamond"),
        };
        [
            (HotbarSlot::FIRST.inventory_slot(), sword),
            (InventorySlot::HEAD, armor_piece(armor, "helmet")),
            (InventorySlot::CHEST, armor_piece(armor, "chestplate")),
            (InventorySlot::LEGS, armor_piece(armor, "leggings")),
            (InventorySlot::FEET, armor_piece(armor, "boots")),
        ]
    }

    /// The fighters' inventory: the kit in its slots, everything else empty.
    pub fn loadout(self) -> Option<[ItemStack; InventorySlot::COUNT]> {
        let mut loadout = std::array::from_fn(|_| ItemStack::default());
        for (slot, item) in self.items() {
            let id = ITEMS.iter().position(|(name, ..)| *name == item)?;
            loadout[slot.index()] = ItemStack::plain(id as i32, 1).ok()?;
        }
        Some(loadout)
    }
}

fn armor_piece(material: &str, piece: &str) -> &'static str {
    match (material, piece) {
        ("iron", "helmet") => "minecraft:iron_helmet",
        ("iron", "chestplate") => "minecraft:iron_chestplate",
        ("iron", "leggings") => "minecraft:iron_leggings",
        ("iron", _) => "minecraft:iron_boots",
        (_, "helmet") => "minecraft:diamond_helmet",
        (_, "chestplate") => "minecraft:diamond_chestplate",
        (_, "leggings") => "minecraft:diamond_leggings",
        _ => "minecraft:diamond_boots",
    }
}

/// Stone up to y = 63 over chunks -4..4 on both axes, spawn at the origin:
/// room for the whole layout.
#[cfg(test)]
pub(crate) fn test_world() -> World {
    use halcyon_world::loader::LoadedChunk;
    use halcyon_world::{LoadedWorld, SECTION_COUNT, Section};
    let chunks = (-4..4)
        .flat_map(|x| (-4..4).map(move |z| (x, z)))
        .map(|(x, z)| {
            let mut sections =
                Box::new(std::array::from_fn::<_, SECTION_COUNT, _>(|_| Section::Uniform(0)));
            sections[7] = Section::Uniform(1);
            LoadedChunk {
                block_entities: vec![],
                x,
                z,
                data: vec![],
                heights: [0; 37],
                wire_bytes: 0,
                sections,
            }
        })
        .collect();
    World::from_loaded(LoadedWorld {
        chunks,
        spawn: (0.5, 64.0, 0.5, 0.0, 0.0),
        missing_blocks: 0,
        remapped_biomes: 0,
    })
}

pub struct Boxes {
    kind: &'static str,
    layout: Layout,
    loadout: [ItemStack; InventorySlot::COUNT],
    /// Fighters still standing in each box.
    alive: [BTreeSet<i32>; BOXES],
}

impl Boxes {
    pub const KIND: &str = "boxes";
    /// The same game in tougher armor with a weaker sword.
    pub const LONG_KIND: &str = "boxes_long";

    pub fn new(world: &World, kind: &'static str, kit: Kit) -> Option<Self> {
        let layout = Layout::new(world)?;
        Some(Self { kind, layout, loadout: kit.loadout()?, alive: Default::default() })
    }
}

impl Match for Boxes {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn rules(&self) -> MatchRules {
        MatchRules { min_players: 2, max_players: CAPACITY, countdown_seconds: 5, end_seconds: 5 }
    }

    fn on_create(&mut self, ctx: &mut GameCtx<'_>) {
        if let Err(error) = ctx.set_blocks(&self.layout.cells) {
            eprintln!("instance {} (boxes): arena build failed: {error:?}", ctx.instance());
        }
    }

    fn on_arrive(&mut self, ctx: &mut GameCtx<'_>, player: i32, _waiting: usize) {
        let gallery = self.layout.gallery;
        ctx.teleport(player, gallery);
        ctx.set_rules(player, PlayerRules::lobby(gallery));
    }

    fn on_start(&mut self, ctx: &mut GameCtx<'_>, roster: &[i32]) {
        let mut filled = [0; BOXES];
        for (&player, index) in roster.iter().zip(pairing(roster.len())) {
            let spawn = self.layout.spawn(index, filled[index]);
            filled[index] += 1;
            ctx.set_rules(player, PlayerRules::open(spawn));
            // Before the reset: a fresh life drops actions queued for the old one.
            ctx.set_loadout(player, self.loadout.clone(), HotbarSlot::FIRST);
            ctx.reset(player);
            self.alive[index].insert(player);
        }
        ctx.broadcast(super::FIGHT);
    }

    fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {
        let (player, killer) = match event {
            GameEvent::Left(player) => (player, None),
            GameEvent::Died { player, killer } => (player, killer),
            GameEvent::Respawned(_) => return,
        };
        let Some(alive) = self.alive.iter_mut().find(|alive| alive.contains(&player)) else {
            return;
        };
        alive.remove(&player);
        if matches!(event, GameEvent::Left(_)) {
            return;
        }
        let name = ctx.name(player).unwrap_or("?").to_owned();
        match killer.and_then(|killer| ctx.name(killer)).map(str::to_owned) {
            Some(killer) => ctx.broadcast(&format!("{name} was eliminated by {killer}.")),
            None => ctx.broadcast(&format!("{name} was eliminated.")),
        }
        // Eliminated players respawn on the gallery to watch.
        ctx.set_rules(player, PlayerRules::lobby(self.layout.gallery));
    }

    fn tick(&mut self, _ctx: &mut GameCtx<'_>) -> Option<Outcome> {
        self.alive
            .iter()
            .all(|alive| alive.len() <= 1)
            .then(|| Outcome { winners: self.alive.iter().flatten().copied().collect() })
    }
}

#[cfg(test)]
#[path = "../tests/tests_boxes.rs"]
mod tests;
