//! Closed-loop execution of one edge. The same controller validates edges
//! (with ideal hands) and drives bots at runtime (through the motor), so a
//! validated edge is a promise about exactly this behaviour.

use super::stand::centre;
use super::{Cell, Edge, EdgeKind, Stand, StandKind};
use crate::bot::motor::{Intent, wrap_degrees};
use crate::lobby::Transform;

/// What the bot observes about itself each tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Obs {
    pub position: Transform,
    /// Displacement over the previous tick.
    pub velocity: [f64; 3],
}

/// Per-execution parameters: the bot's skill, or validation perturbations.
#[derive(Clone, Copy, Debug)]
pub struct Execution {
    pub aim_tolerance: f32,
    pub hesitation: u16,
    pub yaw_offset: f32,
    pub takeoff_offset: f32,
}

impl Execution {
    pub const IDEAL: Execution =
        Execution { aim_tolerance: 0.5, hesitation: 0, yaw_offset: 0.0, takeoff_offset: 0.0 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Fell = 0,
    Missed = 1,
    Timeout = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Done,
    Failed(Failure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Approach,
    Align,
    Execute,
    Air,
}

const APPROACH_TOLERANCE: f64 = 0.08;
const STILL: f64 = 0.03;
const EYE_HEIGHT: f64 = 1.62;
/// A player overlaps a block until its centre is half a width past the rim.
const LANDING_MARGIN: f64 = 0.29;
/// Arrival radius when stepping onto the current cell's centre.
pub const CENTRED: f64 = 0.3;
/// How far a walk may stray from the line between its cells.
const CORRIDOR: f64 = 1.3;
/// Jump before this tick's motion would carry the centre this far past the rim.
const RIM_SAFETY: f64 = 0.27;

pub struct EdgeRun {
    pub from: Cell,
    pub edge: Edge,
    from_feet: f64,
    origin: [f64; 2],
    target: [f64; 2],
    dir: [f64; 2],
    dir_yaw: f32,
    rim: f64,
    from_kind: StandKind,
    exec: Execution,
    phase: Phase,
    ticks: u16,
    waited: u16,
    jumped_at: Option<u16>,
    airborne: bool,
    /// Closest horizontal approach to the target centre, for diagnostics.
    pub closest: f64,
}

/// Minecraft yaw facing horizontal direction `dir`.
pub fn yaw_of(dir: [f64; 2]) -> f32 {
    (-dir[0]).atan2(dir[1]).to_degrees() as f32
}

fn ladder_wall(facing: u8) -> [f64; 2] {
    // The wall is opposite the facing: north faces -z, so its wall is +z.
    match facing {
        0 => [0.0, 1.0],
        1 => [-1.0, 0.0],
        2 => [0.0, -1.0],
        _ => [1.0, 0.0],
    }
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

impl EdgeRun {
    pub fn new(from: Cell, from_stand: &Stand, edge: Edge, exec: Execution) -> Self {
        let origin = centre(from);
        let target = centre(edge.to);
        let delta = sub(target, origin);
        let length = delta[0].hypot(delta[1]);
        let dir = if length > 1e-6 {
            [delta[0] / length, delta[1] / length]
        } else if let StandKind::Ladder(facing) = from_stand.kind {
            ladder_wall(facing)
        } else {
            [0.0, 1.0]
        };
        Self {
            from,
            edge,
            from_feet: from_stand.feet_y,
            origin,
            target,
            dir,
            dir_yaw: yaw_of(dir),
            rim: from_stand.rim(dir),
            from_kind: from_stand.kind,
            exec,
            phase: if edge.kind == EdgeKind::Jump {
                Phase::Approach
            } else {
                Phase::Execute
            },
            ticks: 0,
            waited: 0,
            jumped_at: None,
            airborne: false,
            closest: f64::INFINITY,
        }
    }

    pub fn dir_yaw(&self) -> f32 {
        self.dir_yaw
    }

    fn limit(&self) -> u16 {
        self.edge.ticks.saturating_mul(3).saturating_add(60).saturating_add(self.exec.hesitation)
    }

    /// Whether `position` rests at this edge's destination: on its floor and
    /// still overlapping its block, even if the centre overhangs the rim.
    pub fn arrived(&self, position: Transform) -> bool {
        let [x, y, z] = self.edge.to;
        if self.edge.kind == EdgeKind::Climb {
            return position.x.floor() as i32 == x
                && position.z.floor() as i32 == z
                && position.y >= f64::from(y) - 1e-3
                && position.y < f64::from(y) + 1.0;
        }
        if self.edge.to == self.from {
            // Stepping onto the current cell: arrive near its centre.
            let [cx, cz] = self.target;
            return position.on_ground
                && (position.y - self.edge.to_feet).abs() < 0.3
                && (position.x - cx).hypot(position.z - cz) < CENTRED;
        }
        let over = |p: f64, cell: i32| {
            p >= f64::from(cell) - LANDING_MARGIN && p <= f64::from(cell) + 1.0 + LANDING_MARGIN
        };
        position.on_ground
            && (position.y - self.edge.to_feet).abs() < 0.3
            && over(position.x, x)
            && over(position.z, z)
    }

    fn look(&self, position: Transform, yaw: f32) -> Intent {
        let to = sub(self.target, [position.x, position.z]);
        let horizontal = to[0].hypot(to[1]).max(0.5);
        let rise = self.edge.to_feet - (position.y + EYE_HEIGHT);
        let pitch = (-rise).atan2(horizontal).to_degrees().clamp(-30.0, 60.0) as f32;
        Intent { yaw: wrap_degrees(yaw), pitch, ..Intent::default() }
    }

    fn towards_target(&self, position: Transform) -> f32 {
        let to = sub(self.target, [position.x, position.z]);
        if to[0].hypot(to[1]) < 1e-3 {
            self.dir_yaw
        } else {
            yaw_of(to)
        }
    }

    pub fn tick(&mut self, obs: &Obs) -> (Intent, Status) {
        let position = obs.position;
        let here = [position.x, position.z];
        let to_target = sub(self.target, here);
        self.closest = self.closest.min(to_target[0].hypot(to_target[1]));
        self.ticks = self.ticks.saturating_add(1);
        let idle = self.look(position, position.yaw);
        if self.arrived(position) && (self.edge.kind != EdgeKind::Jump || self.airborne) {
            return (idle, Status::Done);
        }
        if position.y < self.from_feet.min(self.edge.to_feet) - 1.5 {
            return (idle, Status::Failed(Failure::Fell));
        }
        if self.ticks > self.limit() {
            return (idle, Status::Failed(Failure::Timeout));
        }
        match self.edge.kind {
            EdgeKind::Walk | EdgeKind::Drop => self.walk(obs),
            EdgeKind::Climb => {
                // Ladders climb by pushing into their wall; vines have no
                // wall to push, so hold jump and stay inside the vine.
                let ladder = matches!(self.from_kind, StandKind::Ladder(_));
                let intent =
                    Intent { forward: ladder, jump: true, ..self.look(position, self.dir_yaw) };
                (intent, Status::Running)
            }
            EdgeKind::Jump => self.jump(obs),
        }
    }

    fn walk(&mut self, obs: &Obs) -> (Intent, Status) {
        let position = obs.position;
        // Leaving the corridor between the two cells means the walk failed.
        let offset = sub([position.x, position.z], self.origin);
        let length = sub(self.target, self.origin);
        let span = dot(length, length).max(1e-9);
        let t = (dot(offset, length) / span).clamp(0.0, 1.0);
        let nearest = [self.origin[0] + length[0] * t, self.origin[1] + length[1] * t];
        let away = sub([position.x, position.z], nearest);
        if position.on_ground && away[0].hypot(away[1]) > CORRIDOR {
            return (self.look(position, position.yaw), Status::Failed(Failure::Missed));
        }
        let mut intent = self.look(position, self.towards_target(position));
        if position.on_ground || self.edge.kind == EdgeKind::Walk {
            intent.forward = true;
            intent.sprint = self.edge.plan.sprint;
            // Leaving a climbable upwards: keep climbing until over the ledge.
            intent.jump = self.from_kind.climbable() && self.edge.to_feet > position.y + 0.1;
            if self.edge.to == self.from {
                // Centring on the current cell: coast in rather than orbit.
                let to = sub(self.target, [position.x, position.z]);
                let speed = obs.velocity[0].hypot(obs.velocity[2]);
                intent.forward = to[0].hypot(to[1]) > speed * 2.5 + 0.15;
            }
        } else {
            self.air_control(obs, &mut intent);
        }
        (intent, Status::Running)
    }

    fn jump(&mut self, obs: &Obs) -> (Intent, Status) {
        let position = obs.position;
        let offset = sub([position.x, position.z], self.origin);
        let along = dot(offset, self.dir);
        // Unit vector to the player's left when facing `dir`.
        let left = [self.dir[1], -self.dir[0]];
        let lateral = dot(offset, left);
        let speed = obs.velocity[0].hypot(obs.velocity[2]);
        let mut intent = self.look(position, self.dir_yaw);
        match self.phase {
            Phase::Approach => {
                let error = along + f64::from(self.edge.plan.runup);
                let moving = dot([obs.velocity[0], obs.velocity[2]], self.dir);
                // Press only when not already coasting onto the mark.
                if error > APPROACH_TOLERANCE && moving * -4.0 < error {
                    intent.backward = true;
                } else if error < -APPROACH_TOLERANCE && moving * 4.0 < -error {
                    intent.forward = true;
                }
                if lateral > APPROACH_TOLERANCE {
                    intent.right = true;
                } else if lateral < -APPROACH_TOLERANCE {
                    intent.left = true;
                }
                let placed = error.abs() <= APPROACH_TOLERANCE * 1.5
                    && lateral.abs() <= APPROACH_TOLERANCE * 1.5
                    && speed < STILL;
                if placed || (self.ticks > 40 && error.abs() < 0.3 && lateral.abs() < 0.3) {
                    self.phase = Phase::Align;
                }
                if !position.on_ground {
                    return (intent, Status::Failed(Failure::Fell));
                }
                (intent, Status::Running)
            }
            Phase::Align => {
                self.waited = self.waited.saturating_add(1);
                let aimed =
                    wrap_degrees(position.yaw - self.dir_yaw).abs() <= self.exec.aim_tolerance;
                if aimed && self.waited >= self.exec.hesitation && speed < STILL {
                    self.phase = Phase::Execute;
                    return self.jump(obs);
                }
                (intent, Status::Running)
            }
            Phase::Execute => {
                if !position.on_ground {
                    // Walked off the rim without jumping: let the fall play out.
                    self.airborne = true;
                    self.phase = Phase::Air;
                    return self.jump(obs);
                }
                intent.yaw = wrap_degrees(self.dir_yaw + self.exec.yaw_offset);
                intent.precise = true;
                intent.forward = true;
                intent.sprint = self.edge.plan.sprint;
                let takeoff = f64::from(self.edge.plan.takeoff + self.exec.takeoff_offset);
                let moving = dot([obs.velocity[0], obs.velocity[2]], self.dir);
                // Never run off the support while waiting for the takeoff mark.
                let leaving = along + moving * 1.2 > self.rim + RIM_SAFETY;
                if along >= takeoff || leaving {
                    intent.jump = true;
                    self.jumped_at = Some(self.ticks);
                    self.phase = Phase::Air;
                }
                (intent, Status::Running)
            }
            Phase::Air => {
                if !position.on_ground {
                    self.airborne = true;
                }
                if position.on_ground && self.airborne {
                    return (intent, Status::Failed(Failure::Missed));
                }
                if !self.airborne && self.jumped_at.is_some_and(|at| self.ticks > at + 3) {
                    // The jump never left the ground (head blocked, jump delay).
                    return (intent, Status::Failed(Failure::Missed));
                }
                intent.yaw = wrap_degrees(self.towards_target(position) + self.exec.yaw_offset);
                intent.precise = true;
                intent.sprint = self.edge.plan.sprint;
                if self.airborne {
                    self.air_control(obs, &mut intent);
                } else {
                    intent.forward = true;
                }
                (intent, Status::Running)
            }
        }
    }

    /// Steer the remaining glide onto the target: horizontal speed decays by
    /// 0.91 per airborne tick, so predict where an unpowered glide lands.
    fn air_control(&self, obs: &Obs, intent: &mut Intent) {
        let position = obs.position;
        let to = sub(self.target, [position.x, position.z]);
        let remaining = to[0].hypot(to[1]);
        let heading = if remaining > 1e-6 {
            [to[0] / remaining, to[1] / remaining]
        } else {
            self.dir
        };
        let speed = dot([obs.velocity[0], obs.velocity[2]], heading).max(0.0);
        let airtime = airtime(position.y, obs.velocity[1], self.edge.to_feet);
        let glide = speed * 0.91 * (1.0 - 0.91f64.powi(airtime as i32)) / 0.09;
        if glide < remaining - 0.1 {
            intent.forward = true;
        } else if glide > remaining + 0.25 {
            intent.backward = true;
        }
    }
}

/// Stop moving: counter-steer against the current velocity.
pub fn brake(position: Transform, velocity: [f64; 3]) -> Intent {
    let forward =
        [-(position.yaw.to_radians().sin() as f64), position.yaw.to_radians().cos() as f64];
    let left = [forward[1], -forward[0]];
    let ahead = dot([velocity[0], velocity[2]], forward);
    let side = dot([velocity[0], velocity[2]], left);
    Intent {
        yaw: position.yaw,
        pitch: position.pitch,
        forward: ahead < -0.02,
        backward: ahead > 0.02,
        left: side < -0.02,
        right: side > 0.02,
        ..Intent::default()
    }
}

/// Ticks until falling from `y` with vertical speed `vy` reaches `floor`.
fn airtime(mut y: f64, vy: f64, floor: f64) -> u32 {
    // The observed displacement already includes this tick's gravity.
    let mut vy = (vy - 0.08) * 0.98;
    for tick in 0..80 {
        y += vy;
        vy = (vy - 0.08) * 0.98;
        if y <= floor && vy < 0.0 {
            return tick + 1;
        }
    }
    80
}
