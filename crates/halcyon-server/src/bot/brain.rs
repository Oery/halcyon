//! Behaviour: decide what to do, plan routes and follow them. Produces intents
//! for the motor; never touches physics directly.

use std::collections::{HashMap, VecDeque};

use halcyon_world::BlockView;

use super::motor::{Intent, Motor};
use super::nav::follow::{EdgeRun, Execution, Failure, Obs, Status};
use super::nav::search::{Search, SearchStatus, Step};
use super::nav::stand::cell_of;
use super::nav::{Cell, Edge, EdgeKind, NavService};
use super::profile::BotProfile;
use super::rng::Rng;
use crate::lobby::Transform;
use crate::movement::MovementInput;

/// Graph expansions a bot may spend per tick while planning.
const EXPANSIONS_PER_TICK: usize = 2_000;
/// Planning cost added per remembered failure of an edge, in ticks.
const FAILURE_COST: f32 = 60.0;
/// Farther than this from a walk's starting cell, walk onto it first.
const APPROACH_OFFSET: f64 = super::nav::follow::CENTRED + 0.15;
/// Heading error tolerated before using a door, in degrees.
const DOOR_AIM: f32 = 25.0;
/// Ticks to wait for a used door to open before using it again.
const DOOR_RETRY: u16 = 12;
/// Doors are used from at most this far, within the game's reach.
const DOOR_REACH: f64 = 3.0;

/// The first closed door on the line from `position` to `target` within
/// reach, at foot or head height.
fn door_ahead(
    real: &impl BlockView,
    position: Transform,
    target: [f64; 2],
    feet: f64,
) -> Option<Cell> {
    let to = [target[0] - position.x, target[1] - position.z];
    let length = to[0].hypot(to[1]).min(DOOR_REACH);
    if length < 1e-6 {
        return None;
    }
    let dir = [to[0] / to[0].hypot(to[1]), to[1] / to[0].hypot(to[1])];
    let steps = (length * 4.0).ceil() as i32;
    let base = position.y.min(feet).floor() as i32;
    (0..=steps).find_map(|k| {
        let t = length * f64::from(k) / f64::from(steps);
        let x = (position.x + dir[0] * t).floor() as i32;
        let z = (position.z + dir[1] * t).floor() as i32;
        (0..=1)
            .map(|dy| [x, base + dy, z])
            .find(|&[x, y, z]| real.block_at(x, y, z).is_some_and(super::nav::closed_door))
    })
}

/// Failed plans in a row before a stranded bot returns to its checkpoint.
const STRANDED_RESET: u8 = 3;
/// Ticks a bot keeps swimming toward a floor before giving up.
const SWIM_LIMIT: u32 = 600;
/// How far a swimming bot looks for a floor to climb onto.
const SWIM_RADIUS: i32 = 8;
/// A position jump this large is a teleport or respawn, not movement.
const TELEPORT_DISTANCE: f64 = 4.0;

/// `HALCYON_BOT_TRACE` logs every failed edge, for debugging navigation.
fn trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("HALCYON_BOT_TRACE").is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Planning,
    Moving,
    Recovering,
    Finished,
    Stuck,
    Swimming,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Idle(u32),
    Plan,
    Follow,
    Recover(u32),
    Finished,
    Stuck(u32),
    /// Fell into water: swim to the nearest floor, then replan.
    Swim(u32),
}

/// Unhurried looking around between actions.
struct Gaze {
    yaw: f32,
    pitch: f32,
    hold: u32,
}

impl Gaze {
    fn tick(&mut self, rng: &mut Rng, current: f32) -> Intent {
        if self.hold == 0 {
            self.yaw = current + rng.range(-70.0, 70.0) as f32;
            self.pitch = rng.range(-15.0, 25.0) as f32;
            self.hold = rng.ticks(15, 60);
        }
        self.hold -= 1;
        Intent { yaw: self.yaw, pitch: self.pitch, ..Intent::default() }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BrainStats {
    pub falls: u32,
    /// Failed edges by cause: fell, missed, timed out.
    pub failures: [u32; 3],
    pub edges_done: u32,
    pub replans: u32,
    pub finished_at: Option<u64>,
}

pub struct Brain {
    pub profile: BotProfile,
    rng: Rng,
    motor: Motor,
    state: State,
    search: Search,
    path: VecDeque<Step>,
    run: Option<EdgeRun>,
    last: Option<Transform>,
    failures: HashMap<(Cell, Cell), u8>,
    gaze: Gaze,
    /// Destination of the last completed edge: where the route says we are.
    arrived: Option<Cell>,
    /// Index of the next checkpoint to reach.
    checkpoint: usize,
    /// Trapped: return to the last checkpoint.
    reset_requested: bool,
    /// Consecutive plans that found no way on.
    stranded: u8,
    /// Where the last partial route ended, to notice the same dead end.
    dead_end: Option<Cell>,
    /// A door to use this tick.
    pending_use: Option<Cell>,
    /// Ticks before trying a door again.
    door_wait: u16,
    ticks: u64,
    pub stats: BrainStats,
    /// Print this bot's state every second (`HALCYON_BOT_TRACE=<name>`).
    pub traced: Option<String>,
}

impl Brain {
    pub fn new(profile: BotProfile, seed: u64, facing: Transform) -> Self {
        let mut rng = Rng::new(seed);
        let delay = rng.ticks(20, 60);
        Self {
            profile,
            rng,
            motor: Motor::new(facing.yaw, facing.pitch),
            state: State::Idle(delay),
            search: Search::default(),
            path: VecDeque::new(),
            run: None,
            last: None,
            failures: HashMap::new(),
            gaze: Gaze { yaw: facing.yaw, pitch: 0.0, hold: 0 },
            arrived: None,
            checkpoint: 0,
            reset_requested: false,
            stranded: 0,
            dead_end: None,
            pending_use: None,
            door_wait: 0,
            ticks: 0,
            stats: BrainStats::default(),
            traced: None,
        }
    }

    /// Checkpoints reached so far.
    pub fn checkpoint(&self) -> usize {
        self.checkpoint
    }

    /// Where a trapped bot asks to be returned: its last checkpoint, or
    /// `None` for the spawn. Taken once per request.
    pub fn take_reset(&mut self, nav: &NavService) -> Option<Option<Cell>> {
        if !std::mem::take(&mut self.reset_requested) {
            return None;
        }
        Some(self.checkpoint.checked_sub(1).and_then(|index| nav.checkpoints.get(index).copied()))
    }

    /// The next checkpoint, or the final goal once every checkpoint is behind.
    fn target(&self, nav: &NavService) -> Option<Cell> {
        nav.checkpoints.get(self.checkpoint).copied().or(nav.goal)
    }

    /// Advance past any upcoming checkpoint at `cell`, skipping missed ones.
    fn pass_checkpoint(&mut self, cell: Option<Cell>, nav: &NavService) -> bool {
        let Some(cell) = cell else { return false };
        let ahead = nav.checkpoints.iter().skip(self.checkpoint).position(|&c| c == cell);
        if let Some(offset) = ahead {
            self.checkpoint += offset + 1;
            self.stranded = 0;
        }
        ahead.is_some()
    }

    pub fn mode(&self) -> Mode {
        match self.state {
            State::Idle(_) => Mode::Idle,
            State::Plan => Mode::Planning,
            State::Follow => Mode::Moving,
            State::Recover(_) => Mode::Recovering,
            State::Finished => Mode::Finished,
            State::Stuck(_) => Mode::Stuck,
            State::Swim(_) => Mode::Swimming,
        }
    }

    fn forget_route(&mut self) {
        self.arrived = None;
        self.search.cancel();
        self.path.clear();
        self.run = None;
    }

    pub fn tick(
        &mut self,
        position: Transform,
        nav: &mut NavService,
        view: &impl BlockView,
    ) -> MovementInput {
        self.ticks += 1;
        let velocity = self.last.map_or([0.0; 3], |last| {
            [position.x - last.x, position.y - last.y, position.z - last.z]
        });
        if velocity[0].hypot(velocity[2]).hypot(velocity[1]) > TELEPORT_DISTANCE {
            self.forget_route();
            self.state = State::Recover(self.rng.ticks(10, 30));
        }
        self.last = Some(position);
        if !matches!(self.state, State::Swim(_)) && super::nav::moves::in_water(view, position) {
            self.forget_route();
            self.state = State::Swim(0);
        }
        if position.on_ground && self.pass_checkpoint(Some(cell_of(position)), nav) {
            // Any route in progress led here; plan toward the next target.
            if matches!(self.state, State::Follow) && self.path.is_empty() {
                self.forget_route();
                self.state = State::Plan;
            }
        }
        if let Some(name) = &self.traced
            && self.ticks.is_multiple_of(20)
        {
            eprintln!(
                "trace {name}: mode={:?} at ({:.1},{:.1},{:.1}) ground={} checkpoint={} target={:?} path={:?} edge={:?} searching={} starved={}",
                self.mode(),
                position.x,
                position.y,
                position.z,
                position.on_ground,
                self.checkpoint,
                self.target(nav),
                self.path.front().map(|step| (
                    self.path.len(),
                    step.from,
                    step.edge.to,
                    step.edge.kind
                )),
                self.run.as_ref().map(|run| (run.from, run.edge.to, run.edge.kind)),
                self.search.is_active(),
                self.search.starved,
            );
        }
        let obs = Obs { position, velocity };
        // Plan as if doors were open; the real view shows which still need opening.
        let planning = super::nav::OpenDoors(view);
        let intent = self.decide(&obs, nav, &planning, view);
        self.motor.tick(intent, self.profile, &mut self.rng)
    }

    fn idle(&mut self, position: Transform) -> Intent {
        self.gaze.tick(&mut self.rng, position.yaw)
    }

    fn decide(
        &mut self,
        obs: &Obs,
        nav: &mut NavService,
        view: &impl BlockView,
        real: &impl BlockView,
    ) -> Intent {
        let position = obs.position;
        match self.state {
            State::Idle(ticks) | State::Stuck(ticks) => {
                self.state = if ticks == 0 {
                    State::Plan
                } else if matches!(self.state, State::Idle(_)) {
                    State::Idle(ticks - 1)
                } else {
                    State::Stuck(ticks - 1)
                };
                self.idle(position)
            }
            State::Swim(ticks) => self.swim(position, nav, view, ticks),
            State::Recover(ticks) => {
                if position.on_ground {
                    self.state = if ticks == 0 {
                        State::Plan
                    } else {
                        State::Recover(ticks - 1)
                    };
                }
                self.idle(position)
            }
            State::Finished => {
                if let Some(goal) = nav.goal
                    && position.y < f64::from(goal[1]) - 2.0
                {
                    self.state = State::Plan;
                }
                self.idle(position)
            }
            State::Plan => self.plan(position, nav, view),
            State::Follow => self.follow(obs, nav, view, real),
        }
    }

    /// The standable cell the bot occupies, tolerating overhanging edges.
    fn locate(
        &self,
        position: Transform,
        nav: &mut NavService,
        view: &impl BlockView,
    ) -> Option<Cell> {
        let cell = cell_of(position);
        if let Some(stand) = nav.stand(view, cell)
            && (stand.kind.climbable() || (stand.feet_y - position.y).abs() < 0.3)
        {
            return Some(cell);
        }
        let mut best: Option<(f64, Cell)> = None;
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=1 {
                    let near = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                    let Some(stand) = nav.stand(view, near) else { continue };
                    if (stand.feet_y - position.y).abs() >= 0.3 {
                        continue;
                    }
                    let d = (f64::from(near[0]) + 0.5 - position.x)
                        .hypot(f64::from(near[2]) + 0.5 - position.z);
                    if d < 1.2 && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, near));
                    }
                }
            }
        }
        best.map(|(_, cell)| cell)
    }

    fn plan(&mut self, position: Transform, nav: &mut NavService, view: &impl BlockView) -> Intent {
        let Some(goal) = self.target(nav) else {
            self.state = State::Idle(200);
            return self.idle(position);
        };
        if !self.search.is_active() {
            if !position.on_ground {
                return self.idle(position);
            }
            let Some(start) = self.locate(position, nav, view) else {
                self.state = State::Stuck(self.rng.ticks(20, 60));
                return self.idle(position);
            };
            if self.pass_checkpoint(Some(start), nav) {
                return self.idle(position);
            }
            if start == goal {
                self.finish();
                return self.idle(position);
            }
            self.search.start(start, goal);
        }
        let risk = self.profile.risk_weight();
        let failures = &self.failures;
        let cost = |from: Cell, edge: &Edge| {
            risk * (1.0 - edge.robustness)
                + failures.get(&(from, edge.to)).map_or(0.0, |&n| f32::from(n) * FAILURE_COST)
        };
        let status = self.search.step(nav, view, EXPANSIONS_PER_TICK, &cost);
        let partial = matches!(status, SearchStatus::Partial(_));
        match status {
            SearchStatus::Running => {}
            SearchStatus::Found(path) | SearchStatus::Partial(path) if !path.is_empty() => {
                // The same dead end again: as stranded as having no route.
                let end = path.last().map(|step| step.edge.to);
                let again = partial && end == self.dead_end;
                self.dead_end = if partial { end } else { None };
                if again {
                    self.stranded = self.stranded.saturating_add(1);
                    if self.stranded >= STRANDED_RESET {
                        self.stranded = 0;
                        self.dead_end = None;
                        self.reset_requested = true;
                        self.state = State::Stuck(self.rng.ticks(20, 60));
                        return self.idle(position);
                    }
                }
                if let Some(name) = &self.traced {
                    eprintln!(
                        "trace {name}: route of {} steps to {:?} explored={:?}",
                        path.len(),
                        path.last().map(|step| step.edge.to),
                        self.search.explored()
                    );
                }
                self.path = path.into();
                self.state = State::Follow;
            }
            status => {
                if let Some(name) = &self.traced {
                    let start = self.locate(position, nav, view);
                    eprintln!(
                        "trace {name}: search failed {:?} from {start:?} explored={:?}",
                        matches!(status, SearchStatus::Unreachable),
                        self.search.explored()
                    );
                    for cell in start.into_iter().flat_map(|s| {
                        (-1..=1).flat_map(move |dx| {
                            (-1..=1).map(move |dz| [s[0] + dx, s[1], s[2] + dz])
                        })
                    }) {
                        let Some(candidates) = nav.candidates(view, cell) else {
                            continue;
                        };
                        for c in candidates.iter() {
                            eprintln!(
                                "  {cell:?} -> {:?} {:?} cached={:?}",
                                c.to,
                                c.kind,
                                nav.graph
                                    .checked(cell, c)
                                    .map(|e| e.map(|e| (e.robustness, e.plan)))
                            );
                        }
                    }
                }
                // Stranded where no route leads on: after a few tries, go
                // back to the checkpoint like a player who fell would.
                self.stranded = self.stranded.saturating_add(1);
                if self.stranded >= STRANDED_RESET {
                    self.stranded = 0;
                    self.reset_requested = true;
                }
                self.state = State::Stuck(self.rng.ticks(100, 300));
            }
        }
        self.idle(position)
    }

    /// Head for the nearest floor at about water level, holding jump to stay
    /// afloat and to climb out.
    fn swim(
        &mut self,
        position: Transform,
        nav: &mut NavService,
        view: &impl BlockView,
        ticks: u32,
    ) -> Intent {
        if position.on_ground && !super::nav::moves::in_water(view, position) {
            self.state = State::Recover(self.rng.ticks(5, 20));
            return self.idle(position);
        }
        if ticks > SWIM_LIMIT {
            // Trapped in water: ask the game to return us to our checkpoint,
            // as the course does for players who fall.
            self.reset_requested = true;
            self.state = State::Stuck(self.rng.ticks(20, 60));
            return self.idle(position);
        }
        self.state = State::Swim(ticks + 1);
        let here = cell_of(position);
        let mut best: Option<(f64, [f64; 2])> = None;
        for dx in -SWIM_RADIUS..=SWIM_RADIUS {
            for dz in -SWIM_RADIUS..=SWIM_RADIUS {
                for dy in -1..=2 {
                    let cell = [here[0] + dx, here[1] + dy, here[2] + dz];
                    let Some(stand) = nav.stand(view, cell) else { continue };
                    if stand.kind.climbable() || stand.feet_y > position.y + 1.3 {
                        continue;
                    }
                    let centre = [f64::from(cell[0]) + 0.5, f64::from(cell[2]) + 0.5];
                    let d = (centre[0] - position.x).hypot(centre[1] - position.z);
                    if best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, centre));
                    }
                }
            }
        }
        let Some((_, [x, z])) = best else {
            // Nothing nearby: tread water and look around.
            return Intent { jump: true, ..self.idle(position) };
        };
        let yaw = super::nav::follow::yaw_of([x - position.x, z - position.z]);
        Intent { yaw, pitch: 10.0, forward: true, jump: true, ..Intent::default() }
    }

    /// Face a closed door in the way and use it, as a player would; retry
    /// after a pause if it stays closed (someone may have shut it again).
    fn open_door(&mut self, position: Transform, door: Cell) -> Intent {
        let to = [f64::from(door[0]) + 0.5 - position.x, f64::from(door[2]) + 0.5 - position.z];
        let yaw = super::nav::follow::yaw_of(to);
        let rise = f64::from(door[1]) + 0.5 - (position.y + 1.62);
        let pitch = (-rise).atan2(to[0].hypot(to[1]).max(0.3)).to_degrees() as f32;
        self.door_wait = self.door_wait.saturating_sub(1);
        let aimed = super::motor::wrap_degrees(position.yaw - yaw).abs() < DOOR_AIM;
        if aimed && self.door_wait == 0 {
            self.pending_use = Some(door);
            self.door_wait = DOOR_RETRY;
        }
        Intent { yaw, pitch, ..Intent::default() }
    }

    /// A door the bot asked to use this tick. Taken once.
    pub fn take_use(&mut self) -> Option<Cell> {
        self.pending_use.take()
    }

    fn finish(&mut self) {
        self.state = State::Finished;
        if self.stats.finished_at.is_none() {
            self.stats.finished_at = Some(self.ticks);
        }
    }

    fn follow(
        &mut self,
        obs: &Obs,
        nav: &mut NavService,
        view: &impl BlockView,
        real: &impl BlockView,
    ) -> Intent {
        let position = obs.position;
        // At most one finished edge hands over to the next within a tick.
        for _ in 0..2 {
            if self.run.is_none() {
                let Some(step) = self.path.pop_front() else {
                    let here = self.arrived.or_else(|| self.locate(position, nav, view));
                    if self.pass_checkpoint(here, nav) {
                        self.state = State::Plan;
                    } else if self.checkpoint >= nav.checkpoints.len()
                        && nav.goal.is_some()
                        && here == nav.goal
                    {
                        self.finish();
                    } else {
                        // A partial route ended: plan the next stretch.
                        self.state = State::Plan;
                    }
                    return self.idle(position);
                };
                let Some(stand) = nav.stand(view, step.from) else {
                    self.forget_route();
                    self.state = State::Plan;
                    return self.idle(position);
                };
                let [cx, cz] = super::nav::stand::centre(step.from);
                let offset = (position.x - cx).hypot(position.z - cz);
                let step = if !step.edge.kind.is_jump()
                    && step.edge.to != step.from
                    && offset > APPROACH_OFFSET
                    && position.on_ground
                {
                    // Standing off the step's cell (an overhang, a spawn
                    // offset): walk onto it first, then take the step.
                    self.path.push_front(step);
                    Step {
                        from: step.from,
                        edge: Edge {
                            to: step.from,
                            to_feet: stand.feet_y,
                            kind: EdgeKind::Walk,
                            plan: super::nav::Plan::WALK,
                            ticks: 10,
                            robustness: 1.0,
                            via: None,
                            damage: 0.0,
                        },
                    }
                } else {
                    step
                };
                let hesitation = if step.edge.kind.is_jump() {
                    self.profile.hesitation(step.edge.robustness, &mut self.rng)
                } else {
                    0
                };
                let exec = Execution {
                    aim_tolerance: self.profile.aim_tolerance(),
                    approach_tolerance: self.profile.approach_tolerance(),
                    hesitation,
                    ..Execution::IDEAL
                };
                self.run = Some(EdgeRun::new(step.from, &stand, step.edge, exec));
            }
            let run = self.run.as_mut().expect("edge run exists");
            if let Some(door) = door_ahead(real, position, run.target(), run.edge.to_feet) {
                return self.open_door(position, door);
            }
            let run = self.run.as_mut().expect("edge run exists");
            let (intent, status) = run.tick(obs);
            match status {
                Status::Running => return intent,
                Status::Done => {
                    self.stats.edges_done += 1;
                    self.arrived = self.run.take().map(|run| run.edge.to);
                }
                Status::Failed(failure) => {
                    let key = (run.from, run.edge.to);
                    let count = self.failures.entry(key).or_default();
                    *count = count.saturating_add(1);
                    if failure != Failure::Timeout {
                        self.stats.falls += 1;
                    }
                    self.stats.failures[failure as usize] += 1;
                    self.stats.replans += 1;
                    if trace() {
                        eprintln!(
                            "bot: {failure:?} {:?} {:?} -> {:?} robustness={} plan={:?} at ({:.2},{:.2},{:.2}) ground={}",
                            run.edge.kind,
                            run.from,
                            run.edge.to,
                            run.edge.robustness,
                            run.edge.plan,
                            position.x,
                            position.y,
                            position.z,
                            position.on_ground,
                        );
                    }
                    self.forget_route();
                    self.state = State::Recover(self.rng.ticks(10, 40));
                    return self.idle(position);
                }
            }
        }
        self.idle(position)
    }
}
