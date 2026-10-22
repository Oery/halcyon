//! Java 26.3 ground, water and lava travel in discrete 50-ms movement units.
//! Defaults only: food is fixed, item use/effects/attribute modifiers are disabled.
use std::sync::OnceLock;

use super::*;

#[cfg(test)]
#[path = "../tests/tests_movement_physics.rs"]
mod tests;

const GRAVITY: f64 = 0.08;
const AIR_DRAG: f64 = 0.98f32 as f64;
static SIN: OnceLock<Box<[f32]>> = OnceLock::new();

#[cfg(test)]
pub fn shared_motion_bytes() -> usize {
    SIN.get().map_or(0, |table| std::mem::size_of_val(table.as_ref()))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MovementInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jumping: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    pub yaw: f32,
    pub pitch: f32,
}

impl MovementInput {
    pub fn from_flags(flags: u8, yaw: f32, pitch: f32) -> Self {
        Self {
            forward: flags & 1 != 0,
            backward: flags & 2 != 0,
            left: flags & 4 != 0,
            right: flags & 8 != 0,
            jumping: flags & 16 != 0,
            sneaking: flags & 32 != 0,
            sprinting: flags & 64 != 0,
            yaw,
            pitch,
        }
    }

    fn direction(self, sneaking: bool) -> [f64; 2] {
        let mut x = f32::from(i8::from(self.left) - i8::from(self.right));
        let mut z = f32::from(i8::from(self.forward) - i8::from(self.backward));
        let length = (x * x + z * z).sqrt();
        if length == 0.0 {
            return [0.0; 2];
        }
        // KeyboardInput normalizes first; LocalPlayer then applies slowdown and
        // maps the input square to a unit circle. Preserve float operation order.
        x /= length;
        z /= length;
        x *= 0.98;
        z *= 0.98;
        if sneaking {
            x *= 0.3;
            z *= 0.3;
        }
        let length = (x * x + z * z).sqrt();
        x *= 1.0 / length;
        z *= 1.0 / length;
        let ratio = x.abs().min(z.abs()) / x.abs().max(z.abs());
        let speed = (length * (1.0 + ratio * ratio).sqrt()).min(1.0);
        let [mut x, mut z] = [f64::from(x * speed), f64::from(z * speed)];
        let squared = x * x + z * z;
        if squared > 1.0 {
            let length = squared.sqrt();
            x /= length;
            z /= length;
        }
        [x, z]
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MotionState {
    /// Blocks per vanilla movement tick, not blocks per owner invocation.
    pub velocity: [f64; 3],
    jump_delay: u8,
    pub(super) berry_stuck: bool,
}

/// One shared lookup table matches Java Mth's quantized trigonometry. It is
/// initialized once, not retained or recomputed separately for every player.
fn sin_cos(yaw: f32) -> [f64; 2] {
    let table = SIN.get_or_init(|| {
        (0..65_536)
            .map(|i| (f64::from(i) * std::f64::consts::PI * 2.0 / 65_536.0).sin() as f32)
            .collect()
    });
    let angle = f64::from(yaw * (std::f32::consts::PI / 180.0)) * 10_430.378_350_470_453;
    [
        f64::from(table[(angle as i64 & 65_535) as usize]),
        f64::from(table[((angle + 16_384.0) as i64 & 65_535) as usize]),
    ]
}

fn influencing_surface(terrain: &impl Terrain, position: Transform, depth: f32) -> MotionInfo {
    let support = position.on_ground.then(|| ground_contact(terrain, position)).flatten();
    let mut block = support
        .map_or([position.x.floor() as i32, 0, position.z.floor() as i32], |contact| contact.block);
    let tall = support.is_some()
        && terrain
            .cell(block[0], block[1], block[2])
            .is_some_and(|cell| cell.shapes.iter().any(|shape| shape.max[1] > 1.0));
    if !tall {
        block[1] = (position.y - f64::from(depth)).floor() as i32;
    }
    terrain.cell(block[0], block[1], block[2]).map_or_else(MotionInfo::default, |cell| cell.motion)
}

fn feet_surface(terrain: &impl Terrain, position: Transform) -> MotionInfo {
    terrain
        .cell(position.x.floor() as i32, position.y.floor() as i32, position.z.floor() as i32)
        .map_or_else(MotionInfo::default, |cell| cell.motion)
}

impl MovementState {
    /// Advances owner-driven motion without requiring a client position packet.
    /// Humans continue through `advance`: applying physics again to their
    /// predictions would double gravity, friction, and surface impulses.
    pub fn step(&mut self, world: &impl BlockView, input: MovementInput) -> Advance {
        self.step_with(world, input)
    }

    fn blocked(&mut self, cells: usize, unsupported: bool) -> Advance {
        self.motion = MotionState::default();
        let mut result = self.resolve(&EmptyTerrain, std::iter::empty(), false, cells);
        result.correction = true;
        result.uncertain_motion = unsupported;
        result
    }

    fn step_with(&mut self, terrain: &impl Terrain, input: MovementInput) -> Advance {
        if !input.yaw.is_finite() || !input.pitch.is_finite() {
            return self.blocked(0, false);
        }
        let standing = player_box(self.transform, false);
        let mut query = standing;
        query.min[1] -= STEP_HEIGHT + POSITION_TOLERANCE;
        query.max[1] += STEP_HEIGHT;
        let mut cells = 0;
        match collect_colliders(
            terrain,
            query,
            inflate(player_box_with_height(self.transform, pose_height(self.pose)), 0.02),
            &mut self.colliders,
            &mut cells,
        ) {
            Some(context) if !context.unsupported => {}
            context => return self.blocked(cells, context.is_some_and(|c| c.unsupported)),
        }
        let Some(fluids) = fluid_contact(terrain, self.transform, self.pose, true, &mut cells)
        else {
            return self.blocked(cells, false);
        };
        let Fluids { water, lava, high } = fluids;
        let wants_sprint = input.sprinting && input.forward && !input.backward;
        let swimming =
            wants_sprint && water.touching() && (self.swimming || (water.eyes && water.feet));
        let pose = select_pose(self.transform, input.sneaking, swimming, &self.colliders);
        let crouching = pose == PlayerPose::Crouching;
        let slow = crouching || (pose == PlayerPose::Swimming && !water.touching());
        self.transform.on_ground &= supported(
            player_box_with_height(self.transform, pose_height(pose)),
            POSITION_TOLERANCE,
            &self.colliders,
        );
        let ground = influencing_surface(terrain, self.transform, 0.500001);
        let feet = feet_surface(terrain, self.transform);
        let sprinting = wants_sprint
            && (!slow
                || water.eyes
                || (pose == PlayerPose::Swimming && self.sprinting && !input.sneaking));
        let mut velocity = self.motion.velocity;
        water.push(&mut velocity, WATER_CURRENT);
        lava.push(&mut velocity, LAVA_CURRENT);
        if horizontal_distance(velocity) < 9e-6 {
            velocity[0] = 0.0;
            velocity[2] = 0.0;
        }
        if velocity[1].abs() < 0.003 {
            velocity[1] = 0.0;
        }
        self.motion.jump_delay = self.motion.jump_delay.saturating_sub(1);
        if !input.jumping {
            self.motion.jump_delay = 0;
        }
        let [sin, cos] = sin_cos(input.yaw);
        if water.touching() && input.sneaking {
            velocity[1] -= f64::from(0.04f32);
        }
        // LivingEntity.aiStep measures water depth with lava's height when both touch.
        let depth = if lava.touching() { lava.height } else { water.height };
        if input.jumping
            && ((water.touching() && (!self.transform.on_ground || depth > FLUID_JUMP_THRESHOLD))
                || (lava.touching()
                    && (!self.transform.on_ground || lava.height > FLUID_JUMP_THRESHOLD)))
        {
            velocity[1] += f64::from(0.04f32);
        } else if input.jumping && self.transform.on_ground && self.motion.jump_delay == 0 {
            let jump = 0.42f32 * if feet.jump == 1.0 { ground.jump } else { feet.jump };
            velocity[1] = velocity[1].max(f64::from(jump));
            if sprinting {
                velocity[0] -= sin * 0.2;
                velocity[2] += cos * 0.2;
            }
            self.motion.jump_delay = 10;
        }
        let friction = if self.transform.on_ground { ground.friction } else { 1.0 };
        let speed = if fluids.touching() {
            0.02
        } else if self.transform.on_ground {
            let speed = if sprinting { 0.13f32 } else { 0.1f32 };
            if f64::from(friction) > 0.6 {
                speed * (0.21600002 / (friction * friction * friction))
            } else {
                speed
            }
        } else if sprinting {
            0.025999999
        } else {
            0.02
        };
        let [x, z] = input.direction(slow).map(|n| n * f64::from(speed));
        velocity[0] += x * cos - z * sin;
        velocity[2] += z * cos + x * sin;
        if swimming {
            let look_y = -sin_cos(input.pitch)[0];
            if look_y <= 0.0 || input.jumping || high {
                velocity[1] += (look_y - velocity[1]) * if look_y < -0.2 { 0.085 } else { 0.06 };
            }
        }
        // LivingEntity.handleOnClimbable runs only on the dry travel path.
        if !fluids.touching() && climbing(terrain, self.transform) {
            let limit = f64::from(0.15f32);
            velocity[0] = velocity[0].clamp(-limit, limit);
            velocity[2] = velocity[2].clamp(-limit, limit);
            velocity[1] = velocity[1].max(-limit);
            if velocity[1] < 0.0 && input.sneaking {
                velocity[1] = 0.0;
            }
        }
        // LivingEntity.travelInFluid samples falling before the move.
        let falling = velocity[1] <= 0.0;
        // Entity.move consumes last tick's inside-bush multiplier and clears
        // momentum before travel applies drag/gravity to the remaining velocity.
        let movement = if std::mem::take(&mut self.motion.berry_stuck) {
            let movement = [
                velocity[0] * f64::from(0.8f32),
                velocity[1] * 0.75,
                velocity[2] * f64::from(0.8f32),
            ];
            velocity = [0.0; 3];
            movement
        } else {
            velocity
        };
        let old = self.transform;
        let proposal = MovementProposal {
            transform: Transform {
                x: old.x + movement[0],
                y: old.y + movement[1],
                z: old.z + movement[2],
                yaw: input.yaw,
                pitch: input.pitch,
                on_ground: false,
            },
            sneaking: input.sneaking,
            sprinting,
        };
        let mut result = self.resolve(terrain, std::iter::once(proposal), false, cells);
        if result.correction {
            self.motion = MotionState::default();
            return result;
        }
        for axis in [0, 2] {
            if result.collisions[axis] {
                velocity[axis] = 0.0;
            }
        }
        if result.collisions[1] {
            let bounce = if result.transform.on_ground && velocity[1] < -GRAVITY && !input.sneaking
            {
                f64::from(result.ground.bounce)
            } else {
                0.0
            };
            if bounce > 0.0 {
                // Entity.move restitution applies before either travel drag.
                let fraction = (result.transform.y - old.y) / velocity[1];
                velocity[1] = (fraction * GRAVITY - velocity[1])
                    * (1.0 + fraction * (AIR_DRAG - 1.0))
                    * bounce;
            } else {
                velocity[1] = 0.0;
            }
        }
        if fluids.touching() {
            if water.touching() {
                let drag = f64::from(if sprinting { 0.9f32 } else { 0.8f32 });
                velocity[0] *= drag;
                velocity[2] *= drag;
                velocity[1] *= f64::from(0.8f32);
                velocity[1] = fluid_falling_adjusted(velocity[1], falling, sprinting);
            } else if lava.height <= FLUID_JUMP_THRESHOLD {
                velocity[0] *= 0.5;
                velocity[2] *= 0.5;
                velocity[1] *= f64::from(0.8f32);
                velocity[1] = fluid_falling_adjusted(velocity[1], falling, sprinting);
            } else {
                velocity = velocity.map(|n| n * 0.5);
            }
            if !water.touching() {
                velocity[1] -= GRAVITY / 4.0;
            }
            if result.collisions[0] || result.collisions[2] {
                self.jump_out_of_fluid(terrain, &mut result, old, &mut velocity);
            }
            self.motion.velocity = velocity;
            return result;
        }
        if (result.collisions[0] || result.collisions[2] || input.jumping)
            && climbing(terrain, result.transform)
        {
            velocity[1] = 0.2;
        }
        let ground_after = influencing_surface(terrain, result.transform, 0.500001);
        let feet_after = feet_surface(terrain, result.transform);
        let speed_factor = f64::from(if feet_after.speed == 1.0 {
            ground_after.speed
        } else {
            feet_after.speed
        });
        let drag = f64::from(friction * 0.91f32);
        velocity[0] *= speed_factor * drag;
        velocity[2] *= speed_factor * drag;
        velocity[1] = (velocity[1] - GRAVITY) * AIR_DRAG;
        if honey_slide(terrain, result.transform, result.pose, (velocity[1] / AIR_DRAG) + GRAVITY) {
            let old_y = velocity[1] / AIR_DRAG + GRAVITY;
            if old_y < -0.13 {
                let scale = -0.05 / old_y;
                velocity[0] *= scale;
                velocity[2] *= scale;
            }
            velocity[1] = (-0.05 - GRAVITY) * AIR_DRAG;
            self.fall_distance = 0.0;
        }
        if result.transform.on_ground
            && result.ground.surface == Surface::Slime
            && !input.sneaking
            && velocity[1].abs() < 0.1
        {
            let scale = 0.4 + velocity[1].abs() * 0.2;
            velocity[0] *= scale;
            velocity[2] *= scale;
        }
        self.motion.velocity = velocity;
        result
    }
}

/// Eye heights of every supported pose are at least 0.4.
const FLUID_JUMP_THRESHOLD: f64 = 0.4;

/// LivingEntity.getFluidFallingAdjustedMovement with default gravity.
fn fluid_falling_adjusted(velocity_y: f64, falling: bool, sprinting: bool) -> f64 {
    if sprinting {
        velocity_y
    } else if falling
        && (velocity_y - 0.005).abs() >= 0.003
        && (velocity_y - GRAVITY / 16.0).abs() < 0.003
    {
        -0.003
    } else {
        velocity_y - GRAVITY / 16.0
    }
}

impl MovementState {
    /// LivingEntity.jumpOutOfFluid: a horizontally blocked swimmer climbs out
    /// when the raised destination is free of collision and of any fluid.
    fn jump_out_of_fluid(
        &self,
        terrain: &impl Terrain,
        result: &mut Advance,
        old: Transform,
        velocity: &mut [f64; 3],
    ) {
        let delta = [
            velocity[0],
            velocity[1] + f64::from(0.6f32) - result.transform.y + old.y,
            velocity[2],
        ];
        let bounds =
            translate(player_box_with_height(result.transform, pose_height(result.pose)), delta);
        if self.colliders.iter().any(|shape| intersects(bounds, *shape)) {
            return;
        }
        let destination = Transform {
            x: result.transform.x + delta[0],
            y: result.transform.y + delta[1],
            z: result.transform.z + delta[2],
            ..result.transform
        };
        if let Some(fluids) =
            fluid_contact(terrain, destination, result.pose, false, &mut result.collision_cells)
            && !fluids.touching()
        {
            velocity[1] = f64::from(0.3f32);
        }
    }
}

struct EmptyTerrain;
impl Terrain for EmptyTerrain {
    fn contains_column(&self, _: i32, _: i32) -> bool {
        false
    }
    fn cell(&self, _: i32, _: i32, _: i32) -> Option<TerrainCell> {
        None
    }
}
