//! Move candidates from geometry, and their validation by simulation.
//!
//! Candidates are cheap and optimistic; a search validates only those it
//! actually pursues, so most never cost a simulation.

use halcyon_world::BlockView;

use super::follow::{EdgeRun, Execution, Obs, Status, brake};
use super::stand::centre;
use super::{Cell, Edge, EdgeKind, NavGraph, Plan, Stand, StandKind};
use crate::bot::motor::Intent;
use crate::lobby::Transform;

const STEP: f64 = 0.6;
/// Highest rise a walk is tried for: a stair's full block, in two steps.
const WALK_RISE: f64 = 1.0;
/// Highest a jump can raise the feet (0.42 jump impulse peaks near 1.25).
const MAX_RISE: f64 = 1.3;
const MAX_DROP: f64 = 3.6;
/// Courses drop players onto lower stages; accept losing up to one heart.
const MAX_FALL_DAMAGE: f32 = 2.0;
const MAX_REACH: i32 = 5;
const MAX_JUMP_CANDIDATES: usize = 40;
const SIM_LIMIT: u16 = 200;
const SETTLE_TICKS: u16 = 10;
/// Optimistic speeds for candidate cost estimates, in blocks per tick.
const WALK_SPEED: f32 = 0.3;
const JUMP_SPEED: f32 = 0.45;
const CLIMB_SPEED: f32 = 0.2;
/// Height of a standing jump's apex above the feet.
const JUMP_APEX: f64 = 1.25;
/// Optimistic fraction of a fall's height a slime bounce gives back.
const BOUNCE_RETURN: f64 = 0.9;
const BOUNCE_REACH: i32 = 4;
const MAX_BOUNCE_CANDIDATES: usize = 24;

const NEIGHBOURS: [[i32; 2]; 8] =
    [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]];

/// Generous horizontal reach of a sprint jump with a short run-up.
fn reach(rise: f64) -> f64 {
    if rise > 0.0 {
        4.9 - rise * 0.9
    } else {
        4.9 + (-rise).min(3.0) * 0.4
    }
}

/// Cheap rejection before simulating: no solid block may cut through the
/// body along the straight line between the two feet positions.
fn clear_flight(view: &impl BlockView, from: Cell, here: Stand, to: Cell, there: Stand) -> bool {
    let [ax, az] = centre(from);
    let [bx, bz] = centre(to);
    let distance = (bx - ax).hypot(bz - az);
    let samples = (distance * 4.0).ceil() as i32;
    (1..samples).all(|k| {
        let t = f64::from(k) / f64::from(samples);
        let x = (ax + (bx - ax) * t).floor() as i32;
        let z = (az + (bz - az) * t).floor() as i32;
        let feet = here.feet_y.max(there.feet_y);
        [feet + 0.6, feet + 1.4].into_iter().all(|height| {
            let y = height.floor() as i32;
            view.block_at(x, y, z).is_some_and(|id| {
                // Thin floors such as lily pads and carpets sit below the body.
                halcyon_world::state_info(id).collision_boxes().iter().all(|shape| {
                    f64::from(y) + shape.max[1] < height || f64::from(y) + shape.min[1] > height
                })
            })
        })
    })
}

pub fn in_water(view: &impl BlockView, at: Transform) -> bool {
    view.block_at(at.x.floor() as i32, (at.y + 0.1).floor() as i32, at.z.floor() as i32)
        .is_some_and(|id| halcyon_world::fluid_info(id).kind != halcyon_world::FluidKind::Empty)
}

/// A move suggested by geometry, not yet validated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub to: Cell,
    pub to_feet: f64,
    pub kind: EdgeKind,
    /// Lower bound on the validated duration, in ticks.
    pub estimate: f32,
    /// For bounces: the slime cell to land on first, and its feet height.
    pub via: Option<(Cell, f64)>,
}

/// Result of one simulated execution.
pub struct Outcome {
    pub ticks: Option<u16>,
    /// Closest horizontal approach to the destination centre.
    pub closest: f64,
    /// Furthest progress along the jump line.
    pub furthest: f64,
    /// Fall damage taken on the way.
    pub damage: f32,
}

impl NavGraph {
    /// Run `edge` from rest on `from` with ideal hands and the given
    /// perturbations, then require the player to settle at the destination.
    pub fn simulate(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        from_stand: Stand,
        edge: Edge,
        exec: Execution,
        late_jump: bool,
    ) -> Outcome {
        let mut run = EdgeRun::new(from, &from_stand, edge, exec);
        let [x, z] = centre(from);
        let [lx, lz] = run.left();
        let start = Transform {
            x: x + lx * exec.start_lateral,
            y: from_stand.feet_y,
            z: z + lz * exec.start_lateral,
            yaw: run.dir_yaw(),
            pitch: 0.0,
            on_ground: matches!(from_stand.kind, StandKind::Ground),
        };
        self.sim.place(start);
        self.stats.simulations += 1;
        let mut previous = start;
        let mut velocity = [0.0; 3];
        let mut delayed = false;
        let mut settled: Option<u16> = None;
        let mut damage = 0.0f32;
        let mut ticks = SIM_LIMIT;
        for tick in 0..SIM_LIMIT {
            let position = self.sim.transform();
            let intent = if let Some(done) = settled {
                if tick - done >= SETTLE_TICKS {
                    self.stats.simulated_ticks += u64::from(tick);
                    return Outcome {
                        ticks: Some(done),
                        closest: 0.0,
                        furthest: run.furthest,
                        damage,
                    };
                }
                if !run.arrived(position) {
                    ticks = tick;
                    break;
                }
                // Brake to a stop; on a ladder, sneaking holds position.
                if edge.kind == EdgeKind::Climb {
                    Intent { yaw: position.yaw, sneak: true, ..Intent::default() }
                } else {
                    brake(position, velocity)
                }
            } else {
                let (intent, status) = run.tick(&Obs { position, velocity });
                match status {
                    Status::Running => intent,
                    Status::Done => {
                        settled = Some(tick);
                        continue;
                    }
                    Status::Failed(_) => {
                        ticks = tick;
                        break;
                    }
                }
            };
            let mut input = intent.exact();
            if late_jump {
                if input.jumping && !delayed {
                    input.jumping = false;
                    delayed = true;
                } else if delayed {
                    input.jumping = true;
                    delayed = false;
                }
            }
            let advance = self.sim.step(view, input);
            damage += advance.landed_fall_damage.unwrap_or(0.0);
            let now = self.sim.transform();
            if advance.correction
                || advance.uncertain_motion
                || advance.landed_fall_damage.is_some_and(|damage| damage > MAX_FALL_DAMAGE)
                // Swimming is not planned: a splash is a miss.
                || in_water(view, now)
            {
                ticks = tick;
                break;
            }
            velocity = [now.x - previous.x, now.y - previous.y, now.z - previous.z];
            previous = now;
        }
        self.stats.simulated_ticks += u64::from(ticks);
        Outcome { ticks: None, closest: run.closest, furthest: run.furthest, damage }
    }

    /// Every move worth trying from `from`, from geometry alone.
    pub(super) fn build_candidates(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
    ) -> Vec<Candidate> {
        let mut found: Vec<Candidate> = Vec::new();
        let on_ladder = here.kind.climbable();

        // Walks, step-ups and ladder exits to the eight neighbours.
        for [dx, dz] in NEIGHBOURS {
            let length = if dx != 0 && dz != 0 {
                std::f32::consts::SQRT_2
            } else {
                1.0
            };
            for dy in -1..=2 {
                let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                let Some(there) = self.stand(view, to) else { continue };
                let rise = there.feet_y - here.feet_y;
                // Up to a block: stairs climb by stepping; the simulation decides.
                let limit = if on_ladder { 1.6 } else { WALK_RISE };
                if rise > limit || rise < -STEP {
                    continue;
                }
                found.push(Candidate {
                    to,
                    to_feet: there.feet_y,
                    kind: EdgeKind::Walk,
                    estimate: length / WALK_SPEED,
                    via: None,
                });
            }
        }

        if on_ladder {
            let up = [from[0], from[1] + 1, from[2]];
            if let Some(there) = self.stand(view, up)
                && there.kind.climbable()
            {
                found.push(Candidate {
                    to: up,
                    to_feet: there.feet_y,
                    kind: EdgeKind::Climb,
                    estimate: 1.0 / CLIMB_SPEED,
                    via: None,
                });
            }
        } else {
            // Drops: walk off onto the first floor below a neighbour.
            for [dx, dz] in NEIGHBOURS {
                for dy in (-4..=-1).rev() {
                    let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                    let Some(there) = self.stand(view, to) else { continue };
                    let fall = here.feet_y - there.feet_y;
                    if fall > STEP && fall <= MAX_DROP && !found.iter().any(|c| c.to == to) {
                        found.push(Candidate {
                            to,
                            to_feet: there.feet_y,
                            kind: EdgeKind::Drop,
                            estimate: 1.0 / WALK_SPEED,
                            via: None,
                        });
                    }
                    break;
                }
            }
        }

        if here.kind == StandKind::Ground {
            let jumps = self.jump_candidates(view, from, here, &found);
            found.extend(jumps);
            let bounces = self.bounce_candidates(view, from, here, &found);
            found.extend(bounces);
        }
        found
    }

    /// Floors reachable by landing on a nearby slime block and rebounding.
    fn bounce_candidates(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        existing: &[Candidate],
    ) -> Vec<Candidate> {
        let mut found: Vec<Candidate> = Vec::new();
        for dx in -MAX_REACH..=MAX_REACH {
            for dz in -MAX_REACH..=MAX_REACH {
                let distance = f64::from(dx).hypot(f64::from(dz));
                if distance > f64::from(MAX_REACH) + 0.3 {
                    continue;
                }
                for dy in -6..=1 {
                    let slime = [from[0] + dx, from[1] + dy, from[2] + dz];
                    let below = view.block_at(slime[0], slime[1] - 1, slime[2]);
                    if !below.is_some_and(|id| {
                        halcyon_world::motion_info(id).surface == halcyon_world::Surface::Slime
                    }) {
                        continue;
                    }
                    let Some(pad) = self.stand(view, slime) else { continue };
                    // Jump apex above the pad, mostly returned by the bounce.
                    let fall = here.feet_y + JUMP_APEX - pad.feet_y;
                    let rebound = fall * BOUNCE_RETURN;
                    if rebound <= MAX_RISE {
                        continue;
                    }
                    for tx in -BOUNCE_REACH..=BOUNCE_REACH {
                        for tz in -BOUNCE_REACH..=BOUNCE_REACH {
                            for ty in 1..=rebound.ceil() as i32 {
                                let to = [slime[0] + tx, slime[1] + ty, slime[2] + tz];
                                let Some(there) = self.stand(view, to) else { continue };
                                let rise = there.feet_y - pad.feet_y;
                                if there.kind != StandKind::Ground
                                    || rise <= MAX_RISE
                                    || rise > rebound
                                    || to == from
                                    || existing.iter().chain(&found).any(|c| c.to == to)
                                {
                                    continue;
                                }
                                let span = f64::from(tx).hypot(f64::from(tz));
                                found.push(Candidate {
                                    to,
                                    to_feet: there.feet_y,
                                    kind: EdgeKind::Bounce,
                                    estimate: (distance + span) as f32 / JUMP_SPEED,
                                    via: Some((slime, pad.feet_y)),
                                });
                            }
                        }
                    }
                }
            }
        }
        found.sort_by(|a, b| a.estimate.total_cmp(&b.estimate));
        found.truncate(MAX_BOUNCE_CANDIDATES);
        found
    }

    /// Validate one candidate by simulation; `None` if it cannot be done.
    pub(super) fn validate_candidate(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        candidate: &Candidate,
    ) -> Option<Edge> {
        let to = candidate.to;
        let there = self.stand(view, to)?;
        let mut edge = Edge {
            to,
            to_feet: there.feet_y,
            kind: candidate.kind,
            plan: Plan::WALK,
            ticks: 1,
            robustness: 1.0,
            via: None,
            damage: 0.0,
        };
        match candidate.kind {
            EdgeKind::Jump | EdgeKind::Bounce => {
                self.validate_jump(view, from, here, to, there, candidate.via)
            }
            EdgeKind::Walk if self.trivial_walk(view, from, here, to, there) => {
                edge.ticks = if from[0] != to[0] && from[2] != to[2] { 7 } else { 5 };
                Some(edge)
            }
            _ => {
                let outcome = self.simulate(view, from, here, edge, Execution::IDEAL, false);
                edge.ticks = outcome.ticks?.max(1);
                edge.damage = outcome.damage;
                Some(edge)
            }
        }
    }

    /// Level walks between full floors need no simulation.
    fn trivial_walk(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        to: Cell,
        there: Stand,
    ) -> bool {
        const FULL: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
        let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
        there.feet_y == here.feet_y
            && here.kind == StandKind::Ground
            && there.kind == StandKind::Ground
            && here.extent == FULL
            && there.extent == FULL
            && (dx == 0 || dz == 0 || {
                let a = self.stand(view, [from[0] + dx, from[1], from[2]]);
                let b = self.stand(view, [from[0], from[1], from[2] + dz]);
                a.is_some_and(|s| s.feet_y == here.feet_y)
                    && b.is_some_and(|s| s.feet_y == here.feet_y)
            })
    }

    fn jump_candidates(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        existing: &[Candidate],
    ) -> Vec<Candidate> {
        let mut found = Vec::new();
        for dx in -MAX_REACH..=MAX_REACH {
            for dz in -MAX_REACH..=MAX_REACH {
                let distance = f64::from(dx).hypot(f64::from(dz));
                if distance < 0.5 || distance > f64::from(MAX_REACH) + 0.3 {
                    continue;
                }
                for dy in (-4..=2).rev() {
                    let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                    let Some(there) = self.stand(view, to) else { continue };
                    let rise = there.feet_y - here.feet_y;
                    // A plain step makes a jump pointless; a stair-height
                    // walk may fail validation, so keep a jump in reserve.
                    let walked = existing
                        .iter()
                        .any(|c| c.to == to && (c.kind != EdgeKind::Walk || rise <= STEP));
                    if there.kind != StandKind::Ground || walked {
                        continue;
                    }
                    let adjacent = distance < 1.5;
                    let plausible = if adjacent {
                        rise > STEP && rise <= MAX_RISE
                    } else {
                        (-MAX_DROP..=MAX_RISE).contains(&rise) && distance <= reach(rise)
                    };
                    if plausible
                        && clear_flight(view, from, here, to, there)
                        && (adjacent || !self.walkable_line(view, from, here, to, there))
                    {
                        found.push(Candidate {
                            to,
                            to_feet: there.feet_y,
                            kind: EdgeKind::Jump,
                            estimate: distance as f32 / JUMP_SPEED,
                            via: None,
                        });
                    }
                }
            }
        }
        found.sort_by(|a, b| a.estimate.total_cmp(&b.estimate));
        found.truncate(MAX_JUMP_CANDIDATES);
        found
    }

    /// Whether the straight line between two cells is continuous floor, in
    /// which case walking edges already cover it.
    fn walkable_line(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        to: Cell,
        there: Stand,
    ) -> bool {
        let [ax, az] = centre(from);
        let [bx, bz] = centre(to);
        let distance = (bx - ax).hypot(bz - az);
        let samples = (distance * 2.0).ceil() as i32;
        for k in 1..samples {
            let t = f64::from(k) / f64::from(samples);
            let x = (ax + (bx - ax) * t).floor() as i32;
            let z = (az + (bz - az) * t).floor() as i32;
            let feet = here.feet_y + (there.feet_y - here.feet_y) * t;
            let floor = (-1..=1).any(|dy| {
                self.stand(view, [x, from[1] + dy, z])
                    .is_some_and(|s| s.kind == StandKind::Ground && (s.feet_y - feet).abs() <= STEP)
            });
            if !floor {
                return false;
            }
        }
        true
    }

    fn validate_jump(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        to: Cell,
        there: Stand,
        via: Option<(Cell, f64)>,
    ) -> Option<Edge> {
        let [ax, az] = centre(from);
        // A bounce is lined up on its slime block, not on the final floor.
        let [bx, bz] = centre(via.map_or(to, |(slime, _)| slime));
        let distance = (bx - ax).hypot(bz - az);
        let dir = [(bx - ax) / distance, (bz - az) / distance];
        let rim = here.rim(dir) as f32;
        let back = here.rim([-dir[0], -dir[1]]) as f32;
        let rise = via.map_or(there.feet_y, |(_, feet)| feet) - here.feet_y;
        let long = distance >= 2.5 || (rise > STEP && distance > 1.6);
        let runup_ok = |graph: &mut Self, runup: f32| {
            // Stay on the support while backing up.
            runup <= back + 0.2 && (runup == 0.0 || graph.clear_runup(view, from, here, dir, runup))
        };
        // The farthest-reaching attempt first: if it lands well short, no
        // gentler variant can succeed.
        let farthest = if runup_ok(self, 0.3) { 0.3 } else { 0.0 };
        let mut plans = vec![Plan { runup: farthest, takeoff: rim + 0.2, sprint: true }];
        let sprints: [bool; 2] = if long { [true, false] } else { [false, true] };
        let runups: [f32; 2] = if long { [0.3, 0.0] } else { [0.0, 0.3] };
        let takeoffs: [f32; 3] = if long {
            [rim + 0.2, rim, rim - 0.35]
        } else {
            [rim - 0.35, rim, rim + 0.2]
        };
        for sprint in sprints {
            for runup in runups {
                if !runup_ok(self, runup) {
                    continue;
                }
                for takeoff in takeoffs {
                    let plan = Plan { runup, takeoff: takeoff.max(-0.3), sprint };
                    if !plans.contains(&plan) {
                        plans.push(plan);
                    }
                }
            }
        }
        let mut best: Option<Edge> = None;
        let mut successes = 0;
        for (index, plan) in plans.into_iter().enumerate() {
            let edge = Edge {
                to,
                to_feet: there.feet_y,
                kind: if via.is_some() {
                    EdgeKind::Bounce
                } else {
                    EdgeKind::Jump
                },
                plan,
                ticks: 0,
                robustness: 0.0,
                via,
                damage: 0.0,
            };
            let outcome = self.simulate(view, from, here, edge, Execution::IDEAL, false);
            let Some(ticks) = outcome.ticks else {
                // The farthest-reaching plan fell well short: no gentler
                // plan can do better. Overshooting proves nothing.
                if index == 0 && outcome.closest > 1.0 && outcome.furthest + 1.0 < distance {
                    return None;
                }
                continue;
            };
            let edge = Edge {
                ticks: ticks.max(1),
                robustness: self.robustness(view, from, here, edge),
                damage: outcome.damage,
                ..edge
            };
            successes += 1;
            if best.is_none_or(|b| {
                (edge.robustness, -i32::from(edge.ticks)) > (b.robustness, -i32::from(b.ticks))
            }) {
                best = Some(edge);
            }
            if edge.robustness >= 0.75 || successes >= 3 {
                break;
            }
        }
        best
    }

    fn clear_runup(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        dir: [f64; 2],
        runup: f32,
    ) -> bool {
        let [x, z] = centre(from);
        let at = Transform {
            x: x - dir[0] * f64::from(runup),
            y: here.feet_y,
            z: z - dir[1] * f64::from(runup),
            ..Transform::default()
        };
        self.probe.supports_destination(view, at)
    }

    fn robustness(&mut self, view: &impl BlockView, from: Cell, here: Stand, edge: Edge) -> f32 {
        let trials = [
            (Execution { yaw_offset: 3.0, ..Execution::IDEAL }, false),
            (Execution { yaw_offset: -3.0, ..Execution::IDEAL }, false),
            (Execution { takeoff_offset: -0.15, ..Execution::IDEAL }, false),
            (Execution::IDEAL, true),
            // Lining up a little off the jump line.
            (
                Execution { start_lateral: 0.12, approach_tolerance: 0.15, ..Execution::IDEAL },
                false,
            ),
            (
                Execution { start_lateral: -0.12, approach_tolerance: 0.15, ..Execution::IDEAL },
                false,
            ),
        ];
        let count = trials.len();
        let passed = trials
            .into_iter()
            .filter(|&(exec, late)| {
                self.simulate(view, from, here, edge, exec, late).ticks.is_some()
            })
            .count();
        passed as f32 / count as f32
    }
}
