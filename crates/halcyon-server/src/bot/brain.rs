//! Behaviour: decide what to do, plan routes and follow them. Produces intents
//! for the motor; never touches physics directly.

use std::collections::{HashMap, VecDeque};

use halcyon_world::BlockView;

use super::motor::{Intent, Motor};
use super::nav::follow::{EdgeRun, Execution, Failure, Obs, Status};
use super::nav::search::{JUMP_COST, Search, SearchStatus, Step, heuristic};
use super::nav::stand::cell_of;
use super::nav::{Cell, Edge, EdgeKind, NavService, Validation};
use super::profile::BotProfile;
use super::rng::Rng;
use crate::instance::Transform;
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
/// Ticks a bot may spend somewhere the graph does not know before it
/// returns to its checkpoint.
const LOST_RESET: u16 = 60;
/// Recently explored cells, avoided while exploring so as not to pace.
const RECENT: usize = 8;
/// Exploring cost of revisiting a recent cell, in ticks.
const REVISIT_COST: f32 = 30.0;
/// Exploring drops losing more height than this only as a last resort.
const EXPLORE_DROP: f64 = 1.5;
/// Walks an exploring bot may validate per move past a spent budget.
const EXPLORE_FORCED: usize = 2;
/// Exploring takes less robust edges only as a last resort.
const EXPLORE_ROBUSTNESS: f32 = 0.6;
/// How far a bot that explored off a found route looks for a way back on.
const REJOIN_REACH: i32 = 4;
/// A search started this close to where the bot replans is kept.
const KEEP_SEARCH_REACH: i32 = 8;
/// Planning spent on finding that way back, in expansions.
const REJOIN_EXPANSIONS: usize = 300;
/// Exploring cost that ranks a move behind every ordinary one.
const LAST_RESORT: f32 = 10_000.0;
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
    /// Moving without a known route while one is searched for.
    Exploring,
    Moving,
    Finished,
    Swimming,
    /// In a game, under the fighter brain instead.
    Fighting,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Idle(u32),
    /// No route: locate, start a search and set off exploring.
    Plan,
    Follow,
    Finished,
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
    /// Ticks on the ground with no route or move to follow.
    pub stalled: u32,
    /// Provisional moves taken while no route was known.
    pub explored: u32,
    /// Searches finished, and the ticks they took in all.
    pub searches: u32,
    pub search_ticks: u32,
}

pub struct Brain {
    pub profile: BotProfile,
    rng: Rng,
    motor: Motor,
    state: State,
    search: Search,
    /// Where the running search starts, and its goal.
    anchor: Option<(Cell, Cell)>,
    /// A finished search not yet used: its start, goal and result.
    found: Option<(Cell, Cell, SearchStatus)>,
    path: VecDeque<Step>,
    /// `path` holds exploring moves, not a searched route.
    provisional: bool,
    /// Cells explored lately, newest last.
    recent: VecDeque<Cell>,
    /// Exploring moves to take before searching again.
    explore_left: u8,
    /// Ticks spent where the graph cannot locate the bot.
    lost: u16,
    /// The tick a failed edge was last replanned in.
    replanned_at: u64,
    /// The tick the running search started in.
    search_started: u64,
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
            anchor: None,
            found: None,
            path: VecDeque::new(),
            provisional: false,
            recent: VecDeque::new(),
            explore_left: 0,
            lost: 0,
            replanned_at: 0,
            search_started: 0,
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

    /// Begin as if the first `passed` checkpoints were already behind.
    pub fn start_at(&mut self, passed: usize) {
        self.checkpoint = passed;
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

    /// The target once standing on `cell`: past it if it is an upcoming checkpoint.
    fn target_after(&self, cell: Cell, nav: &NavService) -> Option<Cell> {
        let ahead = nav.checkpoints.iter().skip(self.checkpoint).position(|&c| c == cell);
        let next = self.checkpoint + ahead.map_or(0, |offset| offset + 1);
        nav.checkpoints.get(next).copied().or(nav.goal)
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
            State::Plan => Mode::Exploring,
            State::Follow if self.provisional => Mode::Exploring,
            State::Follow => Mode::Moving,
            State::Finished => Mode::Finished,
            State::Swim(_) => Mode::Swimming,
        }
    }

    /// Stop following the route. A search under way is kept: from nearby,
    /// its result can still be joined.
    fn forget_route(&mut self) {
        self.arrived = None;
        self.path.clear();
        self.provisional = false;
        self.recent.clear();
        self.run = None;
    }

    fn drop_search(&mut self) {
        self.search.cancel();
        self.anchor = None;
        self.found = None;
    }

    /// Search from `from` toward the target the bot will have there.
    fn start_search(&mut self, from: Cell, nav: &NavService) {
        self.drop_search();
        let Some(goal) = self.target_after(from, nav) else {
            return;
        };
        if goal != from {
            self.search.start(from, goal);
            self.anchor = Some((from, goal));
            self.search_started = self.ticks;
        }
    }

    /// Spend this tick's planning on the running search, keeping its result.
    fn advance_search(&mut self, nav: &mut NavService, view: &impl BlockView) {
        let Some((anchor, goal)) = self.anchor.filter(|_| self.search.is_active()) else {
            return;
        };
        let cost = edge_cost(self.profile, &self.failures);
        match self.search.step(nav, view, EXPANSIONS_PER_TICK, &cost) {
            SearchStatus::Running => {}
            status => {
                self.stats.searches += 1;
                self.stats.search_ticks += (self.ticks - self.search_started) as u32;
                self.anchor = None;
                self.found = Some((anchor, goal, status));
            }
        }
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
            self.drop_search();
            self.state = State::Plan;
        }
        self.last = Some(position);
        if !matches!(self.state, State::Swim(_)) && super::nav::moves::in_water(view, position) {
            self.forget_route();
            self.state = State::Swim(0);
        }
        if position.on_ground && self.pass_checkpoint(Some(cell_of(position)), nav) {
            // Searches toward the passed checkpoint are moot.
            let target = self.target(nav);
            let stale = self.anchor.is_some_and(|(_, goal)| Some(goal) != target)
                || self.found.as_ref().is_some_and(|&(_, goal, _)| Some(goal) != target);
            if stale {
                self.drop_search();
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
        if position.on_ground
            && self.run.is_none()
            && matches!(self.state, State::Plan | State::Follow)
        {
            self.stats.stalled += 1;
        }
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
            State::Idle(ticks) => {
                // Think while waiting, to set off with a route.
                if position.on_ground
                    && self.anchor.is_none()
                    && self.found.is_none()
                    && let Some(start) = self.locate(position, nav, view)
                {
                    self.start_search(start, nav);
                }
                self.advance_search(nav, view);
                self.state = if ticks == 0 {
                    State::Plan
                } else {
                    State::Idle(ticks - 1)
                };
                self.idle(position)
            }
            State::Swim(ticks) => {
                self.advance_search(nav, view);
                self.swim(position, nav, view, ticks)
            }
            State::Finished => {
                if let Some(goal) = nav.goal
                    && position.y < f64::from(goal[1]) - 2.0
                {
                    self.state = State::Plan;
                }
                self.idle(position)
            }
            State::Plan => self.plan(obs, nav, view, real),
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
        // Nearby cells at the same height, then within a step: resting on
        // the low half of a stair matches no cell's centre height.
        [0.3, 0.6].into_iter().find_map(|tolerance| {
            let mut best: Option<(f64, Cell)> = None;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    for dy in -1..=1 {
                        let near = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                        let Some(stand) = nav.stand(view, near) else { continue };
                        if (stand.feet_y - position.y).abs() >= tolerance {
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
        })
    }

    /// Set off from wherever the bot stands: search from there, exploring
    /// until the search delivers.
    fn plan(
        &mut self,
        obs: &Obs,
        nav: &mut NavService,
        view: &impl BlockView,
        real: &impl BlockView,
    ) -> Intent {
        let position = obs.position;
        if self.target(nav).is_none() {
            self.state = State::Idle(200);
            return self.idle(position);
        }
        // Plan from the ground, or while holding on to a ladder.
        let climbing = nav.stand(view, cell_of(position)).is_some_and(|s| s.kind.climbable());
        if !position.on_ground && !climbing {
            return self.idle(position);
        }
        let Some(start) = self.locate(position, nav, view) else {
            return self.reorient(position, nav, view);
        };
        self.lost = 0;
        self.pass_checkpoint(Some(start), nav);
        // Keep a search started nearby, such as at the start line or before
        // a fall: the bot can join its route from here.
        let target = self.target(nav);
        let near = |from: Cell, goal: Cell| {
            Some(goal) == target
                && (0..3).all(|axis| (from[axis] - start[axis]).abs() <= KEEP_SEARCH_REACH)
        };
        let kept = self.anchor.is_some_and(|(from, goal)| near(from, goal))
            || self.found.as_ref().is_some_and(|&(from, goal, _)| near(from, goal));
        if !kept {
            self.start_search(start, nav);
        }
        self.path.clear();
        self.run = None;
        self.provisional = false;
        self.arrived = Some(start);
        self.state = State::Follow;
        self.follow(obs, nav, view, real)
    }

    /// Somewhere the graph does not know, such as between two cells: walk
    /// onto the nearest floor, or return to the checkpoint if there is none.
    fn reorient(
        &mut self,
        position: Transform,
        nav: &mut NavService,
        view: &impl BlockView,
    ) -> Intent {
        self.lost += 1;
        if self.lost >= LOST_RESET {
            self.lost = 0;
            self.reset_requested = true;
        }
        let Some([x, z]) = nearest_floor(position, nav, view, 2, 0.6) else {
            return self.idle(position);
        };
        let yaw = super::nav::follow::yaw_of([x - position.x, z - position.z]);
        // Hop if walking there gets nowhere, as players do.
        let jump = self.lost > 10;
        Intent { yaw, pitch: 20.0, forward: true, jump, ..Intent::default() }
    }

    /// Take up a finished search if it leads on from `here`. A failed search
    /// counts towards being stranded and sends the bot exploring.
    fn adopt(&mut self, here: Cell, nav: &mut NavService, view: &impl BlockView) -> bool {
        let Some((anchor, goal, status)) = self.found.take() else {
            return false;
        };
        if Some(goal) != self.target(nav) {
            return false;
        }
        let partial = matches!(status, SearchStatus::Partial(_));
        let path = match status {
            SearchStatus::Found(path) | SearchStatus::Partial(path) if !path.is_empty() => path,
            _ => {
                if let Some(name) = &self.traced {
                    eprintln!(
                        "trace {name}: search from {anchor:?} failed explored={:?}",
                        self.search.explored()
                    );
                }
                // Stranded where no route leads on: after a few tries, go
                // back to the checkpoint like a player who fell would.
                self.strand();
                self.explore_left = self.rng.ticks(2, 5) as u8;
                return false;
            }
        };
        // Exploring may have moved the bot: join the route where it is, or
        // find the way back onto it.
        let (lead, skip) = if here == anchor {
            (Vec::new(), 0)
        } else if let Some(index) = path.iter().position(|step| step.from == here) {
            (Vec::new(), index)
        } else {
            match self.rejoin(here, &path, nav, view) {
                Some(found) => found,
                None => return false,
            }
        };
        // The same dead end again: as stranded as having no route.
        let end = path.last().map(|step| step.edge.to);
        let again = partial && end == self.dead_end;
        self.dead_end = if partial { end } else { None };
        if again && self.strand() {
            self.dead_end = None;
            return false;
        }
        if let Some(name) = &self.traced {
            eprintln!(
                "trace {name}: route of {} steps to {end:?} explored={:?}",
                lead.len() + path.len() - skip,
                self.search.explored()
            );
        }
        self.path = lead.into_iter().chain(path.into_iter().skip(skip)).collect();
        self.provisional = false;
        self.recent.clear();
        // Search the next stretch while this one is walked.
        if let Some(end) = end
            && self.target_after(end, nav).is_some_and(|next| next != end)
        {
            self.start_search(end, nav);
        }
        true
    }

    /// A short way from `here` back onto `path`, and the step to resume from.
    fn rejoin(
        &self,
        here: Cell,
        path: &[Step],
        nav: &mut NavService,
        view: &impl BlockView,
    ) -> Option<(Vec<Step>, usize)> {
        // The nearest route cell, preferring later ones.
        let reach = |cell: Cell| (0..3).map(|axis| (cell[axis] - here[axis]).abs()).max();
        let (index, join) = path
            .iter()
            .enumerate()
            .map(|(index, step)| (index, step.from))
            .filter(|&(_, cell)| reach(cell).is_some_and(|d| d <= REJOIN_REACH))
            .min_by_key(|&(index, cell)| (reach(cell), std::cmp::Reverse(index)))?;
        let mut search = Search::default();
        search.start(here, join);
        let cost = edge_cost(self.profile, &self.failures);
        match search.step(nav, view, REJOIN_EXPANSIONS, &cost) {
            SearchStatus::Found(lead) => Some((lead, index)),
            _ => None,
        }
    }

    /// Count a failure to find a way on; returns whether that asked for a reset.
    fn strand(&mut self) -> bool {
        self.stranded = self.stranded.saturating_add(1);
        if self.stranded < STRANDED_RESET {
            return false;
        }
        self.stranded = 0;
        self.reset_requested = true;
        true
    }

    /// A provisional move out of `here` while no route is known: a plausible
    /// step toward the target, not necessarily a right one.
    fn explore_step(
        &mut self,
        here: Cell,
        nav: &mut NavService,
        view: &impl BlockView,
    ) -> Option<Step> {
        let target = self.target(nav)?;
        let feet = nav.stand(view, here)?.feet_y;
        let candidates = nav.candidates(view, here)?;
        // Most promising first: those get validated past a spent budget.
        let mut candidates: Vec<_> =
            candidates.iter().filter(|candidate| candidate.to != here).copied().collect();
        candidates.sort_by(|a, b| heuristic(a.to, target).total_cmp(&heuristic(b.to, target)));
        let cost = edge_cost(self.profile, &self.failures);
        let mut forced = 0;
        let mut best: Option<(f32, Edge)> = None;
        for candidate in &candidates {
            // Cached edges are free. Jumps cost many simulations: they are
            // left to the search. Walks and drops are cheap: validated while
            // the budget lasts, and a few beyond it.
            let step = matches!(candidate.kind, EdgeKind::Walk | EdgeKind::Drop);
            let edge = match nav.graph.checked(here, candidate) {
                Some(Some(edge)) => edge,
                Some(None) => continue,
                None if !step => continue,
                None => match nav.validate(view, here, candidate) {
                    Validation::Valid(edge) => edge,
                    Validation::Busy if forced < EXPLORE_FORCED => {
                        forced += 1;
                        let Some(edge) = nav.validate_step(view, here, candidate) else {
                            continue;
                        };
                        edge
                    }
                    _ => continue,
                },
            };
            if edge.damage > 0.0 {
                continue;
            }
            let mut score =
                heuristic(edge.to, target) + cost(here, &edge) + self.rng.range(0.0, 10.0) as f32;
            if edge.kind.is_jump() {
                score += JUMP_COST;
            }
            if self.recent.contains(&edge.to) {
                score += REVISIT_COST;
            }
            // Any way will do, but not a long drop or a leap the bot would
            // probably miss.
            if edge.to_feet < feet - EXPLORE_DROP || edge.robustness < EXPLORE_ROBUSTNESS {
                score += LAST_RESORT;
            }
            if best.is_none_or(|(best, _)| score < best) {
                best = Some((score, edge));
            }
        }
        let (_, edge) = best?;
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(edge.to);
        Some(Step { from: here, edge })
    }

    /// The route ran out at an edge boundary: take up a search result, start a
    /// search, or explore meanwhile. `Some` is this tick's intent when there
    /// is nothing to follow.
    fn refill(
        &mut self,
        position: Transform,
        nav: &mut NavService,
        view: &impl BlockView,
    ) -> Option<Intent> {
        let Some(here) = self.arrived.or_else(|| self.locate(position, nav, view)) else {
            self.state = State::Plan;
            return Some(self.reorient(position, nav, view));
        };
        self.lost = 0;
        self.pass_checkpoint(Some(here), nav);
        if self.checkpoint >= nav.checkpoints.len() && nav.goal == Some(here) {
            self.finish();
            return Some(self.idle(position));
        }
        let Some(target) = self.target(nav) else {
            self.state = State::Idle(200);
            return Some(self.idle(position));
        };
        if self.adopt(here, nav, view) {
            return None;
        }
        if self.anchor.is_none() && self.explore_left == 0 {
            self.start_search(here, nav);
            self.advance_search(nav, view);
            if self.adopt(here, nav, view) {
                return None;
            }
        }
        match self.explore_step(here, nav, view) {
            Some(step) => {
                self.path.push_back(step);
                self.provisional = true;
                self.explore_left = self.explore_left.saturating_sub(1);
                self.stats.explored += 1;
                None
            }
            None => {
                // Nothing to try yet: face the way on.
                let [x, z] = super::nav::stand::centre(target);
                let yaw = super::nav::follow::yaw_of([x - position.x, z - position.z]);
                Some(Intent { yaw, pitch: 10.0, ..Intent::default() })
            }
        }
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
            self.state = State::Plan;
            return self.idle(position);
        }
        if ticks > SWIM_LIMIT {
            // Trapped in water: ask the game to return us to our checkpoint,
            // as the course does for players who fall.
            self.reset_requested = true;
            self.state = State::Plan;
            return self.idle(position);
        }
        self.state = State::Swim(ticks + 1);
        let Some([x, z]) = nearest_floor(position, nav, view, SWIM_RADIUS, 1.3) else {
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
        self.advance_search(nav, view);
        // At most one finished edge hands over to the next within a tick.
        for _ in 0..2 {
            if self.run.is_none() {
                if self.path.is_empty()
                    && let Some(intent) = self.refill(position, nav, view)
                {
                    return intent;
                }
                let Some(step) = self.path.pop_front() else {
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
                            then: None,
                            damage: 0.0,
                            onto_ladder: None,
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
                    // Carry on at once from wherever the miss left the bot,
                    // replanning at most once per tick.
                    self.forget_route();
                    self.state = State::Plan;
                    if std::mem::replace(&mut self.replanned_at, self.ticks) == self.ticks {
                        return self.idle(position);
                    }
                    return self.plan(obs, nav, view, real);
                }
            }
        }
        self.idle(position)
    }
}

/// The centre of the nearest floor within `radius` blocks, at most `rise`
/// above the feet, ladders aside.
fn nearest_floor(
    position: Transform,
    nav: &mut NavService,
    view: &impl BlockView,
    radius: i32,
    rise: f64,
) -> Option<[f64; 2]> {
    let here = cell_of(position);
    let mut best: Option<(f64, [f64; 2])> = None;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            for dy in -1..=2 {
                let cell = [here[0] + dx, here[1] + dy, here[2] + dz];
                let Some(stand) = nav.stand(view, cell) else { continue };
                if stand.kind.climbable() || stand.feet_y > position.y + rise {
                    continue;
                }
                let centre = super::nav::stand::centre(cell);
                let d = (centre[0] - position.x).hypot(centre[1] - position.z);
                if best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, centre));
                }
            }
        }
    }
    best.map(|(_, centre)| centre)
}

/// Planning cost of an edge for a bot: its risk aversion and the failures it
/// remembers.
fn edge_cost(
    profile: BotProfile,
    failures: &HashMap<(Cell, Cell), u8>,
) -> impl Fn(Cell, &Edge) -> f32 + '_ {
    let risk = profile.risk_weight();
    move |from, edge| {
        risk * (1.0 - edge.robustness)
            + failures.get(&(from, edge.to)).map_or(0.0, |&n| f32::from(n) * FAILURE_COST)
    }
}
