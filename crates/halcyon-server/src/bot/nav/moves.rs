//! Edge candidates and their validation by simulation.

use halcyon_world::BlockView;

use super::follow::{EdgeRun, Execution, Obs, Status, brake};
use super::stand::centre;
use super::{Cell, Edge, EdgeKind, NavGraph, Plan, Stand, StandKind};
use crate::bot::motor::Intent;
use crate::lobby::Transform;

const STEP: f64 = 0.6;
/// Highest a jump can raise the feet (0.42 jump impulse peaks near 1.25).
const MAX_RISE: f64 = 1.3;
const MAX_DROP: f64 = 3.6;
const MAX_REACH: i32 = 5;
const MAX_JUMP_CANDIDATES: usize = 40;
const SIM_LIMIT: u16 = 200;
const SETTLE_TICKS: u16 = 10;

const NEIGHBOURS: [[i32; 2]; 8] =
    [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]];

/// Generous horizontal reach of a sprint jump with a short run-up.
fn reach(rise: f64) -> f64 {
    if rise > 0.0 {
        4.9 - rise * 1.5
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
        let feet = here.feet_y.max(there.feet_y) + 0.5;
        [feet, feet + 1.0].into_iter().all(|y| {
            view.block_at(x, y.floor() as i32, z)
                .is_some_and(|id| halcyon_world::state_info(id).collision_boxes().is_empty())
        })
    })
}

/// Result of one simulated execution.
pub struct Outcome {
    pub ticks: Option<u16>,
    /// Closest horizontal approach to the destination centre.
    pub closest: f64,
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
        let start = Transform {
            x,
            y: from_stand.feet_y,
            z,
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
        for tick in 0..SIM_LIMIT {
            let position = self.sim.transform();
            let intent = if let Some(done) = settled {
                if tick - done >= SETTLE_TICKS {
                    self.stats.simulated_ticks += u64::from(tick);
                    return Outcome { ticks: Some(done), closest: 0.0 };
                }
                if !run.arrived(position) {
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
                    Status::Failed(_) => break,
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
            if advance.correction
                || advance.uncertain_motion
                || advance.landed_fall_damage.is_some_and(|damage| damage > 0.0)
            {
                break;
            }
            let now = self.sim.transform();
            velocity = [now.x - previous.x, now.y - previous.y, now.z - previous.z];
            previous = now;
        }
        self.stats.simulated_ticks += u64::from(SIM_LIMIT);
        Outcome { ticks: None, closest: run.closest }
    }

    pub(super) fn build_edges(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
    ) -> Vec<Edge> {
        let mut edges: Vec<Edge> = Vec::new();
        let on_ladder = matches!(here.kind, StandKind::Ladder(_));

        // Walks, step-ups and ladder exits to the eight neighbours.
        for [dx, dz] in NEIGHBOURS {
            let diagonal = dx != 0 && dz != 0;
            for dy in -1..=2 {
                let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                let Some(there) = self.stand(view, to) else { continue };
                let rise = there.feet_y - here.feet_y;
                let limit = if on_ladder { 1.6 } else { STEP };
                if rise.abs() > limit || (rise < -STEP) {
                    continue;
                }
                let trivial = rise == 0.0
                    && here.kind == StandKind::Ground
                    && there.kind == StandKind::Ground
                    && here.extent == [0.0, 1.0, 0.0, 1.0]
                    && there.extent == [0.0, 1.0, 0.0, 1.0]
                    && (!diagonal || {
                        let a = self.stand(view, [from[0] + dx, from[1], from[2]]);
                        let b = self.stand(view, [from[0], from[1], from[2] + dz]);
                        a.is_some_and(|s| s.feet_y == here.feet_y)
                            && b.is_some_and(|s| s.feet_y == here.feet_y)
                    });
                let mut edge = Edge {
                    to,
                    to_feet: there.feet_y,
                    kind: EdgeKind::Walk,
                    plan: Plan::WALK,
                    ticks: if diagonal { 7 } else { 5 },
                    robustness: 1.0,
                };
                if !trivial {
                    match self.simulate(view, from, here, edge, Execution::IDEAL, false).ticks {
                        Some(ticks) => edge.ticks = ticks.max(1),
                        None => continue,
                    }
                }
                edges.push(edge);
            }
        }

        // Ladder climbs.
        if on_ladder {
            let up = [from[0], from[1] + 1, from[2]];
            if let Some(there) = self.stand(view, up)
                && matches!(there.kind, StandKind::Ladder(_))
            {
                let edge = Edge {
                    to: up,
                    to_feet: there.feet_y,
                    kind: EdgeKind::Climb,
                    plan: Plan::WALK,
                    ticks: 6,
                    robustness: 1.0,
                };
                if let Some(ticks) =
                    self.simulate(view, from, here, edge, Execution::IDEAL, false).ticks
                {
                    edges.push(Edge { ticks: ticks.max(1), ..edge });
                }
            }
        }

        // Drops: walk off onto the first floor below a neighbour.
        if !on_ladder {
            for [dx, dz] in NEIGHBOURS {
                for dy in (-4..=-1).rev() {
                    let to = [from[0] + dx, from[1] + dy, from[2] + dz];
                    let Some(there) = self.stand(view, to) else { continue };
                    let fall = here.feet_y - there.feet_y;
                    if fall > STEP && fall <= MAX_DROP && !edges.iter().any(|e| e.to == to) {
                        let edge = Edge {
                            to,
                            to_feet: there.feet_y,
                            kind: EdgeKind::Drop,
                            plan: Plan::WALK,
                            ticks: 10,
                            robustness: 1.0,
                        };
                        if let Some(ticks) =
                            self.simulate(view, from, here, edge, Execution::IDEAL, false).ticks
                        {
                            edges.push(Edge { ticks: ticks.max(1), ..edge });
                        }
                    }
                    break;
                }
            }
        }

        if here.kind == StandKind::Ground {
            for (to, there) in self.jump_candidates(view, from, here, &edges) {
                if let Some(edge) = self.validate_jump(view, from, here, to, there) {
                    edges.push(edge);
                }
            }
        }
        edges
    }

    fn jump_candidates(
        &mut self,
        view: &impl BlockView,
        from: Cell,
        here: Stand,
        existing: &[Edge],
    ) -> Vec<(Cell, Stand)> {
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
                    if there.kind != StandKind::Ground || existing.iter().any(|e| e.to == to) {
                        continue;
                    }
                    let rise = there.feet_y - here.feet_y;
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
                        found.push((distance, to, there));
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.truncate(MAX_JUMP_CANDIDATES);
        found.into_iter().map(|(_, to, there)| (to, there)).collect()
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
    ) -> Option<Edge> {
        let [ax, az] = centre(from);
        let [bx, bz] = centre(to);
        let distance = (bx - ax).hypot(bz - az);
        let dir = [(bx - ax) / distance, (bz - az) / distance];
        let rim = here.rim(dir) as f32;
        let back = here.rim([-dir[0], -dir[1]]) as f32;
        let rise = there.feet_y - here.feet_y;
        let long = distance >= 2.5 || (rise > STEP && distance > 1.6);
        let sprints: [bool; 2] = if long { [true, false] } else { [false, true] };
        let runups: [f32; 2] = if long { [0.3, 0.0] } else { [0.0, 0.3] };
        let takeoffs: [f32; 3] = if long {
            [rim + 0.2, rim, rim - 0.35]
        } else {
            [rim - 0.35, rim, rim + 0.2]
        };
        let mut best: Option<Edge> = None;
        let mut successes = 0;
        let mut first = true;
        for sprint in sprints {
            for runup in runups {
                // Stay on the support while backing up.
                if runup > back + 0.2
                    || (runup > 0.0 && !self.clear_runup(view, from, here, dir, runup))
                {
                    continue;
                }
                for takeoff in takeoffs {
                    let edge = Edge {
                        to,
                        to_feet: there.feet_y,
                        kind: EdgeKind::Jump,
                        plan: Plan { runup, takeoff: takeoff.max(-0.3), sprint },
                        ticks: 0,
                        robustness: 0.0,
                    };
                    let outcome = self.simulate(view, from, here, edge, Execution::IDEAL, false);
                    let Some(ticks) = outcome.ticks else {
                        // The most forgiving attempt fell well short: give up early.
                        if first && long && outcome.closest > 1.2 {
                            return None;
                        }
                        first = false;
                        continue;
                    };
                    first = false;
                    let edge = Edge {
                        ticks: ticks.max(1),
                        robustness: self.robustness(view, from, here, edge),
                        ..edge
                    };
                    successes += 1;
                    if best.is_none_or(|b| {
                        (edge.robustness, -i32::from(edge.ticks))
                            > (b.robustness, -i32::from(b.ticks))
                    }) {
                        best = Some(edge);
                    }
                    if edge.robustness >= 1.0 || successes >= 3 {
                        return best;
                    }
                }
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
        ];
        let passed = trials
            .into_iter()
            .filter(|&(exec, late)| {
                self.simulate(view, from, here, edge, exec, late).ticks.is_some()
            })
            .count();
        passed as f32 / 4.0
    }
}
