//! Closed-loop execution of one edge. The same controller validates edges
//! (with steady hands) and drives bots at runtime (through the motor), so a
//! validated edge is a promise about exactly this behaviour.

use super::stand::centre;
use super::{Cell, Edge, EdgeKind, Stand, StandKind};
use crate::bot::motor::{Intent, wrap_degrees};
use crate::bot::profile::BotProfile;
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
    /// How precisely the run-up point must be reached before lining up.
    pub approach_tolerance: f64,
    /// Validation only: start this far to the left of the jump line.
    pub start_lateral: f64,
    pub hesitation: u16,
    pub yaw_offset: f32,
    pub takeoff_offset: f32,
    /// Validation only: whose camera turning the simulated hands have.
    pub hands: BotProfile,
}

impl Execution {
    pub const IDEAL: Execution = Execution {
        aim_tolerance: 0.5,
        approach_tolerance: APPROACH_TOLERANCE,
        start_lateral: 0.0,
        hesitation: 0,
        yaw_offset: 0.0,
        takeoff_offset: 0.0,
        hands: BotProfile { level: 100 },
    };
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
/// How far from a slime pad's centre a landing still counts as on it.
const PAD_CATCH: f64 = 1.5 + LANDING_MARGIN;
/// How far past a slime pad's centre a bounce aims, toward its target.
const PAD_LEAD: f64 = 0.3;
/// Ground covered by a sprint jump at running speed.
const HOP_LENGTH: f64 = 3.6;
/// Where past the rim a momentum hop should land, as the takeoff mark.
const HOP_LANDING: f64 = 0.1;
/// How far a walk may stray from the line between its cells.
const CORRIDOR: f64 = 1.3;
/// Horizontal speed below which a grabbed ladder counts as held.
const LADDER_STILL: f64 = 0.05;
/// Jump before this tick's motion would carry the centre this far past the rim.
const RIM_SAFETY: f64 = 0.29;

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
    /// A momentum hop along the run-up has been jumped.
    hopped: bool,
    airborne: bool,
    /// Feet height of the floor currently aimed at: a bounce's slime first.
    aim_feet: f64,
    /// When a bounce touched its slime block.
    bounced_at: Option<u16>,
    /// Slime pads bounced on so far.
    bounces: u8,
    /// Closest horizontal approach to the target centre, for diagnostics.
    pub closest: f64,
    /// Furthest progress along the line from the source towards the target.
    pub furthest: f64,
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
        let (aim, aim_feet) = edge.via.unwrap_or((edge.to, edge.to_feet));
        let mut target = centre(aim);
        if edge.via.is_some() {
            let next = edge.then.map_or(edge.to, |(pad, _)| pad);
            target = pad_target(aim, next);
        }
        let [lx, lz] = edge.plan.lean;
        target = [target[0] + f64::from(lx), target[1] + f64::from(lz)];
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
            phase: if edge.kind.is_jump() {
                Phase::Approach
            } else {
                Phase::Execute
            },
            ticks: 0,
            waited: 0,
            jumped_at: None,
            hopped: false,
            airborne: false,
            aim_feet,
            bounced_at: None,
            bounces: 0,
            closest: f64::INFINITY,
            furthest: 0.0,
        }
    }

    pub fn dir_yaw(&self) -> f32 {
        self.dir_yaw
    }

    /// Where the edge is currently heading, in world x/z.
    pub fn target(&self) -> [f64; 2] {
        self.target
    }

    /// Unit vector to the left of the jump line, in world x/z.
    pub fn left(&self) -> [f64; 2] {
        [self.dir[1], -self.dir[0]]
    }

    fn limit(&self) -> u16 {
        // Backing up and running in again takes time of its own.
        let runup =
            (self.edge.plan.runup.max(0.0) * 20.0) as u16 + if self.edge.plan.hop { 30 } else { 0 };
        self.edge
            .ticks
            .saturating_mul(3)
            .saturating_add(60)
            .saturating_add(runup)
            .saturating_add(self.exec.hesitation)
    }

    /// Whether `position` rests at this edge's destination: on its floor and
    /// still overlapping its block, even if the centre overhangs the rim.
    pub fn arrived(&self, position: Transform) -> bool {
        let [x, y, z] = self.edge.to;
        if self.edge.kind == EdgeKind::Climb || self.edge.onto_ladder.is_some() {
            // A grab at the cell's edge still carries sideways speed off the
            // ladder: it counts once near the middle.
            let [cx, cz] = centre(self.edge.to);
            let centred = self.edge.onto_ladder.is_none()
                || (position.x - cx).hypot(position.z - cz) < CENTRED;
            return position.x.floor() as i32 == x
                && position.z.floor() as i32 == z
                && position.y >= f64::from(y) - 1e-3
                && position.y < f64::from(y) + 1.0
                && centred;
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
        let rise = self.aim_feet - (position.y + EYE_HEIGHT);
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
        self.furthest = self.furthest.max(dot(sub(here, self.origin), self.dir));
        self.ticks = self.ticks.saturating_add(1);
        let idle = self.look(position, position.yaw);
        let pads = u8::from(self.edge.via.is_some()) + u8::from(self.edge.then.is_some());
        let rebounded = self.bounces == pads;
        // A ladder is held once the grab's drift has died down.
        let held = self.edge.onto_ladder.is_none()
            || obs.velocity[0].hypot(obs.velocity[2]) < LADDER_STILL;
        if self.arrived(position)
            && (!self.edge.kind.is_jump() || self.airborne)
            && rebounded
            && held
        {
            return (idle, Status::Done);
        }
        let pad = self.edge.via.map_or(f64::INFINITY, |(_, feet)| feet);
        let second = self.edge.then.map_or(f64::INFINITY, |(_, feet)| feet);
        let lowest = self.from_feet.min(self.edge.to_feet).min(pad).min(second);
        if position.y < lowest - 1.5 {
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
            EdgeKind::Jump | EdgeKind::Bounce => self.jump(obs),
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
                let tolerance = self.exec.approach_tolerance;
                let error = along + f64::from(self.edge.plan.runup);
                let moving = dot([obs.velocity[0], obs.velocity[2]], self.dir);
                // Press only when not already coasting onto the mark.
                if error > tolerance && moving * -4.0 < error {
                    intent.backward = true;
                } else if error < -tolerance && moving * 4.0 < -error {
                    intent.forward = true;
                }
                if lateral > tolerance {
                    intent.right = true;
                } else if lateral < -tolerance {
                    intent.left = true;
                }
                let placed = error.abs() <= tolerance * 1.5
                    && lateral.abs() <= tolerance * 1.5
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
                if self.edge.plan.hop && self.hopped && !position.on_ground && along < self.rim {
                    // Mid-hop over the run-up: keep running for the real jump.
                    intent.yaw = wrap_degrees(self.dir_yaw + self.exec.yaw_offset);
                    intent.precise = true;
                    intent.forward = true;
                    intent.sprint = true;
                    return (intent, Status::Running);
                }
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
                let leaving = along + moving * 1.05 > self.rim + RIM_SAFETY;
                // Time the hop to land at the rim and jump again at once,
                // before the ground bleeds off the extra speed.
                if self.edge.plan.hop
                    && !self.hopped
                    && along >= self.rim + HOP_LANDING - HOP_LENGTH
                {
                    // The momentum hop, early on the run-up.
                    intent.jump = true;
                    self.hopped = true;
                    return (intent, Status::Running);
                }
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
                    // The pad due now: the first, then any second one.
                    let due = match self.bounces {
                        0 => self.edge.via,
                        1 => self.edge.then,
                        _ => None,
                    };
                    // Pads are often several blocks wide: a neighbouring block
                    // bounces just the same. A plain floor instead fails the
                    // weak-bounce check below.
                    let on_pad = due.is_some_and(|(pad, feet)| {
                        (position.y - feet).abs() < 0.3
                            && (position.x - (f64::from(pad[0]) + 0.5)).abs() < PAD_CATCH
                            && (position.z - (f64::from(pad[2]) + 0.5)).abs() < PAD_CATCH
                    });
                    if !on_pad {
                        return (intent, Status::Failed(Failure::Missed));
                    }
                    // The slime threw us back up: steer for the next pad, or
                    // the real target.
                    self.bounces += 1;
                    self.bounced_at = Some(self.ticks);
                    self.airborne = false;
                    match self.edge.then.filter(|_| self.bounces == 1) {
                        Some((pad, feet)) => {
                            self.target = pad_target(pad, self.edge.to);
                            self.aim_feet = feet;
                        }
                        None => {
                            self.target = centre(self.edge.to);
                            self.aim_feet = self.edge.to_feet;
                        }
                    }
                }
                if !self.airborne && self.bounced_at.is_some_and(|at| self.ticks > at + 3) {
                    // Too weak a bounce to leave the pad.
                    return (intent, Status::Failed(Failure::Missed));
                }
                if !self.airborne
                    && self.bounced_at.is_none()
                    && self.jumped_at.is_some_and(|at| self.ticks > at + 3)
                {
                    // The jump never left the ground (head blocked, jump delay).
                    return (intent, Status::Failed(Failure::Missed));
                }
                intent.yaw = wrap_degrees(self.towards_target(position) + self.exec.yaw_offset);
                intent.precise = true;
                intent.sprint = self.edge.plan.sprint;
                let [x, _, z] = self.edge.to;
                let grabbed = self
                    .edge
                    .onto_ladder
                    .filter(|_| position.x.floor() as i32 == x && position.z.floor() as i32 == z);
                if let Some(facing) = grabbed {
                    // On the ladder: face its wall, climb (pushing into it)
                    // when caught below the target cell, slide down when above
                    // it, sneak to hold on in it, and steer the sideways
                    // drift onto its middle.
                    let wall = ladder_wall(facing);
                    intent.yaw = wrap_degrees(yaw_of(wall));
                    let floor = f64::from(self.edge.to[1]);
                    let below = position.y < floor;
                    let above = position.y >= floor + 1.0;
                    intent.forward = below;
                    intent.sneak = !below && !above;
                    intent.sprint = false;
                    let [cx, cz] = centre(self.edge.to);
                    let side = [wall[1], -wall[0]];
                    let off = dot(sub([position.x, position.z], [cx, cz]), side);
                    let drift = dot([obs.velocity[0], obs.velocity[2]], side);
                    let ahead = off + drift * 3.0;
                    intent.left = ahead < -0.05;
                    intent.right = ahead > 0.05;
                } else if self.airborne {
                    self.air_control(obs, &mut intent);
                } else {
                    intent.forward = true;
                }
                // Hold jump coming down onto a pad, as players do: a jump on
                // slime keeps the rebound and adds the sprint jump's boost,
                // and held early it cannot miss the landing tick.
                let pad_ahead = match self.bounces {
                    0 => self.edge.via,
                    1 => self.edge.then,
                    _ => None,
                };
                let landing = pad_ahead.is_some_and(|(_, feet)| {
                    self.airborne && obs.velocity[1] < 0.0 && position.y - feet < 1.5
                });
                if landing || self.bounced_at == Some(self.ticks) {
                    intent.jump = true;
                    intent.sprint = true;
                }
                if landing {
                    // The landing spot is set by now: face where the rebound
                    // goes next, so the jump's boost carries towards it.
                    let next = match self.bounces {
                        0 => self.edge.then.map_or_else(
                            || centre(self.edge.to),
                            |(pad, _)| pad_target(pad, self.edge.to),
                        ),
                        _ => centre(self.edge.to),
                    };
                    let to = sub(next, [position.x, position.z]);
                    if to[0].hypot(to[1]) > 1e-3 {
                        intent.yaw = wrap_degrees(yaw_of(to) + self.exec.yaw_offset);
                    }
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
        let airtime = airtime(position.y, obs.velocity[1], self.aim_feet);
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

/// Where to land on slime pad `pad`: its far side towards `next`, so the
/// rebound starts closer to it.
fn pad_target(pad: Cell, next: Cell) -> [f64; 2] {
    let target = centre(pad);
    let beyond = sub(centre(next), target);
    let length = beyond[0].hypot(beyond[1]);
    if length > 1e-6 {
        [target[0] + beyond[0] / length * PAD_LEAD, target[1] + beyond[1] / length * PAD_LEAD]
    } else {
        target
    }
}
