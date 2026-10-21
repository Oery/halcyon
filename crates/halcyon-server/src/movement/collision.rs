//! Player bounds, axis clipping, step selection and edge backoff.

use halcyon_world::Aabb;

use super::{EDGE_INCREMENT, EPSILON, HALF_WIDTH, SNEAKING_HEIGHT, STANDING_HEIGHT, STEP_HEIGHT};
use crate::lobby::Transform;

pub(super) fn player_box(position: Transform, sneaking: bool) -> Aabb {
    player_box_with_height(position, if sneaking { SNEAKING_HEIGHT } else { STANDING_HEIGHT })
}

pub(super) fn player_box_with_height(position: Transform, height: f64) -> Aabb {
    Aabb {
        min: [position.x - HALF_WIDTH, position.y, position.z - HALF_WIDTH],
        max: [position.x + HALF_WIDTH, position.y + height, position.z + HALF_WIDTH],
    }
}

pub(super) fn translate(mut bounds: Aabb, delta: [f64; 3]) -> Aabb {
    for (axis, value) in delta.into_iter().enumerate() {
        bounds.min[axis] += value;
        bounds.max[axis] += value;
    }
    bounds
}

pub(super) fn expand_towards(mut bounds: Aabb, delta: [f64; 3]) -> Aabb {
    for (axis, value) in delta.into_iter().enumerate() {
        bounds.min[axis] += value.min(0.0);
        bounds.max[axis] += value.max(0.0);
    }
    bounds
}

pub(super) fn inflate(mut bounds: Aabb, amount: f64) -> Aabb {
    for axis in 0..3 {
        bounds.min[axis] -= amount;
        bounds.max[axis] += amount;
    }
    bounds
}

pub(super) fn intersects(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|axis| a.max[axis] > b.min[axis] + EPSILON && a.min[axis] < b.max[axis] - EPSILON)
}

pub(super) fn supported(bounds: Aabb, depth: f64, colliders: &[Aabb]) -> bool {
    let feet = Aabb {
        min: [bounds.min[0] + EPSILON, bounds.min[1] - depth - EPSILON, bounds.min[2] + EPSILON],
        max: [bounds.max[0] - EPSILON, bounds.min[1] + EPSILON * 2.0, bounds.max[2] - EPSILON],
    };
    colliders.iter().any(|shape| intersects(feet, *shape))
}

pub(super) fn collide(mut bounds: Aabb, mut delta: [f64; 3], colliders: &[Aabb]) -> [f64; 3] {
    let axes = if delta[0].abs() < delta[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    for axis in axes {
        if delta[axis] == 0.0 {
            continue;
        }
        for shape in colliders {
            if (0..3).any(|other| {
                other != axis
                    && (bounds.max[other] <= shape.min[other] + EPSILON
                        || bounds.min[other] >= shape.max[other] - EPSILON)
            }) {
                continue;
            }
            if delta[axis] > 0.0 && bounds.max[axis] <= shape.min[axis] + EPSILON {
                delta[axis] = delta[axis].min((shape.min[axis] - bounds.max[axis]).max(0.0));
            } else if delta[axis] < 0.0 && bounds.min[axis] >= shape.max[axis] - EPSILON {
                delta[axis] = delta[axis].max((shape.max[axis] - bounds.min[axis]).min(0.0));
            }
        }
        bounds.min[axis] += delta[axis];
        bounds.max[axis] += delta[axis];
    }
    delta
}

pub(super) fn horizontal_distance(delta: [f64; 3]) -> f64 {
    delta[0] * delta[0] + delta[2] * delta[2]
}

pub(super) fn try_step(
    bounds: Aabb,
    requested: [f64; 3],
    ordinary: [f64; 3],
    colliders: &[Aabb],
    heights: &mut Vec<f64>,
) -> [f64; 3] {
    let landing = requested[1] < 0.0 && ordinary[1] > requested[1];
    let offset = if landing { ordinary[1] } else { 0.0 };
    let base = translate(bounds, [0.0, offset, 0.0]);
    let mut query = expand_towards(base, [requested[0], STEP_HEIGHT, requested[2]]);
    if !landing {
        query.min[1] -= f64::from(1e-5f32);
    }
    heights.clear();
    for shape in colliders {
        if !intersects(query, *shape) {
            continue;
        }
        for y in [shape.min[1], shape.max[1]] {
            let height = f64::from((y - base.min[1]) as f32);
            if (0.0..=STEP_HEIGHT).contains(&height) && height != f64::from(ordinary[1] as f32) {
                heights.push(height);
            }
        }
    }
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    for &height in heights.iter() {
        let mut candidate = collide(base, [requested[0], height, requested[2]], colliders);
        if horizontal_distance(candidate) > horizontal_distance(ordinary) + EPSILON {
            candidate[1] += offset;
            return candidate;
        }
    }
    ordinary
}

pub(super) fn back_off_edges(bounds: Aabb, delta: &mut [f64; 3], colliders: &[Aabb]) -> bool {
    // The vanilla 0.05 decrement is bounded independently of client coordinates.
    let mut checks = 0;
    for axes in [[true, false], [false, true], [true, true]] {
        loop {
            let x = if axes[0] { delta[0] } else { 0.0 };
            let z = if axes[1] { delta[2] } else { 0.0 };
            if (axes[0] && x == 0.0) || (axes[1] && z == 0.0) {
                break;
            }
            checks += 1;
            if checks > 128 {
                return false;
            }
            if supported(translate(bounds, [x, 0.0, z]), STEP_HEIGHT, colliders) {
                break;
            }
            for axis in [0, 2] {
                if axes[usize::from(axis == 2)] {
                    delta[axis] = if delta[axis].abs() <= EDGE_INCREMENT {
                        0.0
                    } else {
                        delta[axis] - EDGE_INCREMENT.copysign(delta[axis])
                    };
                }
            }
        }
    }
    true
}
