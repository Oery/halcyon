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
/// Deepest walk-off onto a landing that softens falls, such as hay: a
/// player survives about 13 blocks onto hay for one heart.
const SOFT_DROP: i32 = 14;
/// How far past the target a walk around a wall's corner aims.
const LEAN: f32 = 0.7;
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
/// How far below the takeoff a slime pad is looked for: courses drop
/// players several blocks onto a pad to launch them high.
const BOUNCE_DROP: i32 = 10;
const BOUNCE_SEARCH: i32 = 8;
const MAX_BOUNCE_CANDIDATES: usize = 24;
/// Run-ups longer than the own cell, for jumps that need full speed.
const MID_RUNUP: f32 = 1.5;
const LONG_RUNUP: f32 = 2.5;
const RUNWAY_STEP: f32 = 0.1;
/// How far a run-up may start past the floor's back edge: feet overhang the
/// edge as a player's do, short of the 0.3 half-width that still holds them.
const RUNWAY_OVERHANG: f32 = 0.22;
/// Run-up for a hop: room to sprint, jump once and land before the rim.
const HOP_RUNUP: f32 = 5.0;
/// Hop run-ups tried, a fraction of a sprint step apart.
const HOP_PHASES: usize = 12;
/// The shortest runway a hop is tried from.
const MIN_HOP_RUNUP: f32 = 1.5;
/// Run-up shortenings tried after a near miss, a fraction of a sprint step each.
const PHASE_SHIFTS: [f32; 4] = [0.21, 0.14, 0.07, 0.035];

const NEIGHBOURS: [[i32; 2]; 8] =
    [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]];

/// Generous horizontal reach of a sprint jump with a short run-up.
fn reach(rise: f64) -> f64 {
    // A sprint jump at full speed, after a run-up, clears four blocks.
    const FLAT: f64 = 5.3;
    if rise > 0.0 {
        FLAT - rise * 1.1
    } else {
        FLAT + (-rise).min(3.0) * 0.4
    }
}

/// Cheap rejection before simulating: no solid block may cut through the
/// body along the straight line between the two feet positions.
pub(super) fn clear_flight(
    view: &impl BlockView,
    from: Cell,
    here: Stand,
    to: Cell,
    there: Stand,
) -> bool {
    let [ax, az] = centre(from);
    let [bx, bz] = centre(to);
    let distance = (bx - ax).hypot(bz - az);
    let samples = (distance * 4.0).ceil() as i32;
    (1..samples).all(|k| {
        let t = f64::from(k) / f64::from(samples);
        let x = (ax + (bx - ax) * t).floor() as i32;
        let z = (az + (bz - az) * t).floor() as i32;
        // The landing's own column is the destination, not an obstacle:
        // a ladder's plate stands there.
        if [x, z] == [to[0], to[2]] {
            return true;
        }
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
                } else if edge.onto_ladder.is_some() {
                    // A grab still drifts along the ladder: brake it too.
                    Intent { sneak: true, ..brake(position, velocity) }
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
            // Drops: walk off onto the first floor below a neighbour. Deep
            // ones only onto landings that soften the fall; simulation then
            // checks the damage actually taken.
            for [dx, dz] in NEIGHBOURS {
                for dy in (-SOFT_DROP..=-1).rev() {
                    let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                    let Some(there) = self.stand(view, to) else { continue };
                    let fall = here.feet_y - there.feet_y;
                    let reach = fall <= MAX_DROP || soft_landing(view, to, there);
                    if fall > STEP && reach && !found.iter().any(|c| c.to == to) {
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
        for dx in -BOUNCE_SEARCH..=BOUNCE_SEARCH {
            for dz in -BOUNCE_SEARCH..=BOUNCE_SEARCH {
                let distance = f64::from(dx).hypot(f64::from(dz));
                for dy in -BOUNCE_DROP..=1 {
                    // A longer drop onto the pad means longer airtime to cover ground.
                    if distance > reach(f64::from(dy)) + f64::from(-dy).max(0.0) * 0.3 {
                        continue;
                    }
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
            onto_ladder: None,
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
                // A diagonal past a wall's corner may need to lean along it.
                let (dx, dz) = ((to[0] - from[0]) as f32, (to[2] - from[2]) as f32);
                let leans: &[[f32; 2]] = if candidate.kind == EdgeKind::Walk && dx * dz != 0.0 {
                    &[[0.0; 2], [dx * LEAN, 0.0], [0.0, dz * LEAN]]
                } else {
                    &[[0.0; 2]]
                };
                leans.iter().find_map(|&lean| {
                    let edge = Edge { plan: Plan { lean, ..edge.plan }, ..edge };
                    let outcome = self.simulate(view, from, here, edge, Execution::IDEAL, false);
                    Some(Edge { ticks: outcome.ticks?.max(1), damage: outcome.damage, ..edge })
                })
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
            && self.clear_crossing(view, from, to, here.feet_y)
    }

    /// Whether a player fits all along the straight line between two cell
    /// centres: thin blocks such as closed iron doors stand on cell borders,
    /// leaving both cells standable but the crossing blocked.
    fn clear_crossing(&mut self, view: &impl BlockView, from: Cell, to: Cell, feet: f64) -> bool {
        let [ax, az] = centre(from);
        let [bx, bz] = centre(to);
        [0.25, 0.5, 0.75].into_iter().all(|t| {
            let at = Transform {
                x: ax + (bx - ax) * t,
                y: feet,
                z: az + (bz - az) * t,
                ..Transform::default()
            };
            self.probe.supports_destination(view, at)
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
                    // Ladders are jumped at and grabbed, as on ladder courses:
                    // aimed at their top, where they are left, and caught
                    // anywhere below it.
                    let ladder = matches!(there.kind, StandKind::Ladder(_))
                        && !self
                            .stand(view, [to[0], to[1] + 1, to[2]])
                            .is_some_and(|above| above.kind.climbable());
                    if !(there.kind == StandKind::Ground || ladder) || walked {
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
        // Keep level and upward jumps first: from a pillar, the floor below
        // offers dozens of short hops down that would crowd out the course.
        let down = |c: &Candidate| c.to_feet < here.feet_y - STEP;
        found.sort_by(|a, b| down(a).cmp(&down(b)).then(a.estimate.total_cmp(&b.estimate)));
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
        // Backing up along the jump line, over floor at the same height, buys
        // the speed long jumps need.
        let runway = self.runway(view, from, here, dir, back);
        let runup_ok = |runup: f32| runup <= runway;
        let kind = if via.is_some() {
            EdgeKind::Bounce
        } else {
            EdgeKind::Jump
        };
        let edge_for = |plan| Edge {
            to,
            to_feet: there.feet_y,
            kind,
            plan,
            ticks: 0,
            robustness: 0.0,
            via,
            damage: 0.0,
            onto_ladder: ladder_facing(there),
        };
        // Probe with the farthest-reaching plan: if even it lands well short,
        // no plan can succeed. It is a feasibility check, never a preference.
        // The whole runway, short of its very end where a backing bot may
        // slip off, when it is shorter than the long run-up.
        let full = (runway - RUNWAY_STEP).clamp(0.0, LONG_RUNUP);
        let farthest = if long { full } else { full.min(0.3) };
        let hop = long && runway >= MIN_HOP_RUNUP;
        let probe =
            Plan { runup: farthest, takeoff: rim + 0.2, sprint: true, hop: false, lean: [0.0; 2] };
        let probed = self.simulate(view, from, here, edge_for(probe), Execution::IDEAL, false);
        // Only a probe that got past the rim says anything about reach.
        let took_off = probed.furthest > f64::from(rim);
        if probed.ticks.is_none()
            && took_off
            && probed.closest > 1.0
            && probed.furthest + 1.0 < distance
        {
            return None;
        }
        // Simplest plans first, as a player would: short run-ups before long
        // ones, which only win when nothing simpler is as reliable.
        let sprints: [bool; 2] = if long { [true, false] } else { [false, true] };
        let runups: &[f32] = if long {
            &[0.3, 0.0, MID_RUNUP, full, LONG_RUNUP]
        } else {
            &[0.0, 0.3]
        };
        let takeoffs: [f32; 3] = if long {
            [rim + 0.2, rim, rim - 0.35]
        } else {
            [rim - 0.35, rim, rim + 0.2]
        };
        let mut plans = Vec::new();
        for &runup in runups {
            if !runup_ok(runup) {
                continue;
            }
            for sprint in sprints {
                for takeoff in takeoffs {
                    let plan = Plan { takeoff: takeoff.max(-0.3), runup, sprint, ..probe };
                    if !plans.contains(&plan) {
                        plans.push(plan);
                    }
                }
            }
        }
        // A near miss at full speed: the last ground tick fell short of the
        // rim. Shift the run-up by fractions of a sprint step so the takeoff
        // lands later, as players do on max-distance jumps.
        if probed.ticks.is_none() && probe.runup > 1.0 && probed.furthest + 1.0 >= distance {
            plans.extend(PHASE_SHIFTS.map(|shift| Plan { runup: probe.runup - shift, ..probe }));
        }
        // Momentum from a hop is the last resort, for the longest jumps.
        if hop {
            // From as far back as the runway allows: on a short one the hop
            // starts nearly from a stand, as players chain them on small ledges.
            let start = (runway - RUNWAY_STEP).min(HOP_RUNUP);
            let hopping = Plan { runup: start, hop: true, ..probe };
            // The hop must land just short of the rim: try several phases.
            plans.extend(
                (0..HOP_PHASES)
                    .map(|k| Plan { runup: start - k as f32 * 0.07, ..hopping })
                    .filter(|plan| plan.runup >= MIN_HOP_RUNUP),
            );
        }
        let mut best: Option<Edge> = None;
        let mut successes = 0;
        for plan in plans {
            let edge = edge_for(plan);
            let outcome = self.simulate(view, from, here, edge, Execution::IDEAL, false);
            let Some(ticks) = outcome.ticks else { continue };
            let edge = Edge {
                ticks: ticks.max(1),
                robustness: self.robustness(view, from, here, edge),
                damage: outcome.damage,
                ..edge
            };
            successes += 1;
            // More reliable wins; at equal reliability, the shorter run-up.
            let better = best.is_none_or(|b| {
                edge.robustness > b.robustness
                    || (edge.robustness == b.robustness && edge.plan.runup < b.plan.runup)
            });
            if better {
                best = Some(edge);
            }
            if edge.robustness >= 0.75 || successes >= 3 {
                break;
            }
        }
        best
    }

    /// How far back from the centre of `from`, against `dir`, the player can
    /// stand: over floor at the same height with nothing in the way.
    pub(super) fn runway(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        dir: [f64; 2],
        back: f32,
    ) -> f32 {
        let [cx, cz] = centre(from);
        let mut room = 0.0f32;
        // Within the own cell the support's rim bounds it; beyond, floors.
        for step in 1..=(HOP_RUNUP / RUNWAY_STEP) as i32 {
            let distance = step as f32 * RUNWAY_STEP;
            // The front of the footprint is what must stay over the floor.
            let held = f64::from(distance - RUNWAY_OVERHANG);
            let x = cx - dir[0] * held;
            let z = cz - dir[1] * held;
            let cell = [x.floor() as i32, from[1], z.floor() as i32];
            let floor = if cell == from {
                distance <= back + RUNWAY_OVERHANG
            } else {
                self.stand(view, cell).is_some_and(|s| (s.feet_y - here.feet_y).abs() < 0.01)
            };
            if !floor || !self.clear_runup(view, from, here, dir, distance) {
                break;
            }
            room = distance;
        }
        room
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

/// Whether landing in `cell` softens a fall: hay and honey take most of the
/// damage, beds part of the height. Slime is left to bounces.
fn soft_landing(view: &impl BlockView, cell: Cell, stand: Stand) -> bool {
    use halcyon_world::{Surface, motion_info};
    // The block whose top the feet rest on: the cell's own for partial
    // blocks, otherwise the one below.
    let y = if stand.feet_y > f64::from(cell[1]) + 1e-6 {
        cell[1]
    } else {
        cell[1] - 1
    };
    view.block_at(cell[0], y, cell[2]).is_some_and(|id| {
        let info = motion_info(id);
        matches!(info.surface, Surface::Hay | Surface::Honey) || info.fall_reduction > 0.0
    })
}

fn ladder_facing(stand: Stand) -> Option<u8> {
    match stand.kind {
        StandKind::Ladder(facing) => Some(facing),
        _ => None,
    }
}
