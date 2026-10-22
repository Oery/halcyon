//! Bounded inside-block effects, applied after travel in vanilla movement units.
use super::terrain::ShapeContext;
use super::*;

#[cfg(test)]
#[path = "../tests/tests_movement_media.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Stuck {
    #[default]
    None,
    BerryBush,
    Cobweb,
    PowderSnow,
}

impl Stuck {
    pub(super) fn multiplier(self) -> Option<[f64; 3]> {
        match self {
            Self::None => None,
            Self::BerryBush => Some([f64::from(0.8f32), 0.75, f64::from(0.8f32)]),
            Self::Cobweb => Some([0.25, f64::from(0.05f32), 0.25]),
            Self::PowderSnow => Some([f64::from(0.9f32), 1.5, f64::from(0.9f32)]),
        }
    }
}

/// Composition of vertical bubble impulses: clamp(velocity + delta, min, max).
/// Constant storage handles a body touching several column cells in one move.
#[derive(Clone, Copy, Debug)]
pub(super) struct BubbleImpulse {
    delta: f64,
    min: f64,
    max: f64,
}

impl Default for BubbleImpulse {
    fn default() -> Self {
        Self { delta: 0.0, min: f64::NEG_INFINITY, max: f64::INFINITY }
    }
}

impl BubbleImpulse {
    fn push(&mut self, down: bool, above: bool) {
        let (delta, min, max) = match (down, above) {
            (true, false) => (-0.03, -0.3, f64::INFINITY),
            (true, true) => (-0.03, -0.9, f64::INFINITY),
            (false, false) => (0.06, f64::NEG_INFINITY, 0.7),
            (false, true) => (0.1, f64::NEG_INFINITY, 1.8),
        };
        self.delta += delta;
        self.min = (self.min + delta).clamp(min, max);
        self.max = (self.max + delta).clamp(min, max);
    }

    pub(super) fn apply(self, velocity: f64) -> f64 {
        (velocity + self.delta).clamp(self.min, self.max)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct InsideEffects {
    pub(super) stuck: Stuck,
    pub(super) powder_snow: bool,
    pub(super) reset_fall: bool,
    pub(super) bubbles: BubbleImpulse,
}

pub(super) fn has_inside_effects(surface: Surface) -> bool {
    matches!(
        surface,
        Surface::BerryBush | Surface::Cobweb | Surface::PowderSnow | Surface::BubbleColumn { .. }
    )
}

fn swept_contact(bounds: Aabb, delta: [f64; 3], block: Aabb) -> bool {
    let mut enter = 0.0f64;
    let mut exit = 1.0f64;
    for axis in 0..3 {
        let low = block.min[axis] - bounds.max[axis];
        let high = block.max[axis] - bounds.min[axis];
        if delta[axis] == 0.0 {
            if low >= 0.0 || high <= 0.0 {
                return false;
            }
        } else {
            let a = low / delta[axis];
            let b = high / delta[axis];
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
        }
    }
    enter < exit
}

/// Only invoked when collision collection found effect blocks. Every extra
/// read shares the movement budget; diagonal near misses do not trigger effects.
pub(super) fn inside_effects(
    terrain: &impl Terrain,
    bounds: Aabb,
    delta: [f64; 3],
    accepted: Transform,
    context: ShapeContext,
    cells: &mut usize,
) -> Option<InsideEffects> {
    let bounds = inflate(bounds, -f64::from(1e-5f32));
    let end = translate(bounds, delta);
    let query = expand_towards(bounds, delta);
    let min = query.min.map(|n| n.floor() as i32);
    let max = query.max.map(|n| n.floor() as i32);
    let mut effects = InsideEffects::default();
    for y in min[1]..=max[1] {
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                if *cells >= CELLS_PER_STEP {
                    return None;
                }
                *cells += 1;
                let Some(cell) = terrain.cell(x, y, z) else { continue };
                let surface = cell.motion.surface;
                if !has_inside_effects(surface) {
                    continue;
                }
                let origin = [x, y, z].map(f64::from);
                let block = Aabb { min: origin, max: origin.map(|n| n + 1.0) };
                if !swept_contact(bounds, delta, block) {
                    continue;
                }
                match surface {
                    Surface::BerryBush => effects.stuck = Stuck::BerryBush,
                    Surface::Cobweb => effects.stuck = Stuck::Cobweb,
                    Surface::PowderSnow => {
                        let shapes = cell.collision_shapes(context, y);
                        if !shapes.is_empty()
                            && !shapes.iter().any(|shape| {
                                swept_contact(bounds, delta, translate(*shape, origin))
                            })
                        {
                            continue;
                        }
                        // Body contact enables leather-boot climbing; the
                        // movement multiplier additionally requires snowy feet.
                        effects.powder_snow = true;
                        let feet = [accepted.x, accepted.y, accepted.z].map(|n| n.floor() as i32);
                        if *cells >= CELLS_PER_STEP {
                            return None;
                        }
                        *cells += 1;
                        if !terrain
                            .cell(feet[0], feet[1], feet[2])
                            .is_some_and(|c| c.motion.surface == Surface::PowderSnow)
                        {
                            continue;
                        }
                        effects.stuck = Stuck::PowderSnow;
                    }
                    Surface::BubbleColumn { down } => {
                        if delta.iter().map(|n| n * n).sum::<f64>()
                            < (1.0 - f64::from(1e-5f32)).powi(2)
                            && !intersects(end, block)
                        {
                            continue;
                        }
                        if *cells >= CELLS_PER_STEP {
                            return None;
                        }
                        *cells += 1;
                        let above = terrain.cell(x, y + 1, z).is_none_or(|c| {
                            c.shapes.is_empty() && c.fluid.kind == halcyon_world::FluidKind::Empty
                        });
                        effects.bubbles.push(down, above);
                        effects.reset_fall |= !above;
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    effects.reset_fall |= effects.stuck != Stuck::None;
    Some(effects)
}
