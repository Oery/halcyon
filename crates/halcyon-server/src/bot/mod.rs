//! Server-driven participants. A bot owns a lobby `Session` like a human
//! connection and acts only through the shared action path.
//!
//! Layers, bottom-up: `Body` (session bookkeeping a client would do), `motor`
//! (humanlike hands), `nav` (shared terrain graph and route search) and
//! `brain` (what to do next). See `docs/bots.md`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glommio::timer::Timer;
use mc_protocol::packets::Payload;
use mc_protocol::packets::client::PlayerInfoUpdate;
use uuid::Uuid;

use crate::lobby::{Lobby, Profile, Session, Transform};
use crate::movement::MovementInput;
use crate::server::Server;

pub mod brain;
pub mod motor;
pub mod nav;
pub mod profile;
pub mod rng;
#[cfg(test)]
pub mod sim;
#[cfg(test)]
#[path = "../tests/support_bot_map.rs"]
pub mod test_map;

use brain::{Brain, Mode};
use nav::{Budget, NavService};
use profile::BotProfile;
use rng::Rng;

const TICK: Duration = Duration::from_millis(50);
/// Ticks a dead bot waits before pressing respawn.
const RESPAWN_DELAY: u32 = 30;
/// Route validation time shared by all bots per server tick, unless
/// `HALCYON_BOT_NAV_MS` overrides it. Searches pause when it is spent.
const NAV_BUDGET: Duration = Duration::from_millis(4);
/// How far from spawn the climb goal is searched for.
const GOAL_RADIUS: i32 = 128;

/// Live per-bot summaries for diagnostics.
#[derive(Default)]
struct Report {
    bots: Vec<(Mode, f64, [u32; 3], usize)>,
}

/// Spawn `count` bots on the current executor. They share one navigation
/// service, so routes validated for one bot are reused by all.
pub fn spawn(server: Arc<Server>, lobby: Lobby, count: usize, seed: u64) {
    if count == 0 {
        return;
    }
    let limit = std::env::var("HALCYON_BOT_NAV_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(NAV_BUDGET, Duration::from_millis);
    let mut service = NavService::new(Budget::PerWindow { limit, window: TICK });
    service.goal = std::env::var("HALCYON_BOT_GOAL").ok().and_then(|goal| parse_cell(&goal));
    if service.goal.is_none() {
        let (x, y, z, _, _) = server.lobby.spawn;
        let around = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        let start = Instant::now();
        service.goal = lobby.with_registry(|registry| {
            service.detect_goal(&registry.arena.view(&server.lobby), around, GOAL_RADIUS)
        });
        println!("bots: goal {:?} detected in {}ms", service.goal, start.elapsed().as_millis());
    }
    if std::env::var_os("HALCYON_BOT_NO_CHECKPOINTS").is_none() {
        let (x, y, z, _, _) = server.lobby.spawn;
        let around = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        let start = Instant::now();
        service.checkpoints = lobby.with_registry(|registry| {
            service.detect_checkpoints(&registry.arena.view(&server.lobby), around, GOAL_RADIUS)
        });
        println!(
            "bots: {} checkpoints detected in {}ms",
            service.checkpoints.len(),
            start.elapsed().as_millis()
        );
    }
    let nav = Rc::new(RefCell::new(service));
    let report = Rc::new(RefCell::new(Report { bots: vec![(Mode::Idle, 0.0, [0; 3], 0); count] }));
    let (low, high) = profile::parse_levels(std::env::var("HALCYON_BOT_LEVEL").ok().as_deref());
    for index in 0..count {
        let server = server.clone();
        let lobby = lobby.clone();
        let nav = nav.clone();
        let report = report.clone();
        let seed = seed ^ (index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let level = Rng::new(seed ^ 0x1e7e1).ticks(u32::from(low), u32::from(high)) as u8;
        glommio::spawn_local(async move {
            let bot = Bot { index, seed, profile: BotProfile::new(level), nav, report };
            if let Err(error) = run(server, lobby, bot).await {
                eprintln!("bot {index}: {error}");
            }
        })
        .detach();
    }
    if server.diagnostics {
        glommio::spawn_local(report_loop(nav, report)).detach();
    }
}

fn parse_cell(text: &str) -> Option<nav::Cell> {
    let mut parts = text.split(',').map(|part| part.trim().parse::<i32>().ok());
    Some([parts.next()??, parts.next()??, parts.next()??])
}

async fn report_loop(nav: Rc<RefCell<NavService>>, report: Rc<RefCell<Report>>) {
    loop {
        Timer::new(Duration::from_secs(5)).await;
        let report = report.borrow();
        let stats = nav.borrow().stats();
        let count = |mode| report.bots.iter().filter(|(m, ..)| *m == mode).count();
        let mut heights: Vec<f64> = report.bots.iter().map(|(_, y, ..)| *y).collect();
        let mut checkpoints: Vec<usize> = report.bots.iter().map(|(.., c)| *c).collect();
        checkpoints.sort_unstable();
        heights.sort_by(f64::total_cmp);
        let failures = report
            .bots
            .iter()
            .fold([0u32; 3], |sum, (_, _, f, _)| [sum[0] + f[0], sum[1] + f[1], sum[2] + f[2]]);
        eprintln!(
            "bots: idle={} planning={} moving={} recovering={} swimming={} finished={} stuck={} fell={} missed={} timeouts={} checkpoint_median={} checkpoint_max={}/{} y_median={:.1} y_max={:.1} nav_nodes={} nav_edges={} nav_cells={} sims={} sim_ticks={} build_ms={} expansions={}",
            count(Mode::Idle),
            count(Mode::Planning),
            count(Mode::Moving),
            count(Mode::Recovering),
            count(Mode::Swimming),
            count(Mode::Finished),
            count(Mode::Stuck),
            failures[0],
            failures[1],
            failures[2],
            checkpoints.get(checkpoints.len() / 2).copied().unwrap_or(0),
            checkpoints.last().copied().unwrap_or(0),
            nav.borrow().checkpoints.len(),
            heights.get(heights.len() / 2).copied().unwrap_or(0.0),
            heights.last().copied().unwrap_or(0.0),
            stats.nodes,
            stats.edges,
            nav.borrow().graph.cached_cells(),
            stats.simulations,
            stats.simulated_ticks,
            stats.build_time.as_millis(),
            stats.expansions,
        );
    }
}

struct Bot {
    index: usize,
    seed: u64,
    profile: BotProfile,
    nav: Rc<RefCell<NavService>>,
    report: Rc<RefCell<Report>>,
}

async fn run(server: Arc<Server>, lobby: Lobby, bot: Bot) -> Result<(), crate::error::Error> {
    let Bot { index, seed, profile: skill, nav, report } = bot;
    let mut rng = Rng::new(seed);
    let username = format!("Bot{}", index + 1);
    let uuid = Uuid::from_u64_pair(rng.next(), rng.next());
    let mut info = Vec::new();
    PlayerInfoUpdate { uuid, username: &username, properties: vec![] }
        .write_packet(&mut info)
        .await?;
    let profile = Rc::new(Profile { uuid, username, info: Rc::from(info) });

    let (x, y, z, _, _) = server.lobby.spawn;
    let requested = Transform {
        x: x + rng.range(-3.0, 3.0),
        y,
        z: z + rng.range(-3.0, 3.0),
        yaw: rng.range(-180.0, 180.0) as f32,
        pitch: 0.0,
        on_ground: false,
    };
    let spawn = lobby
        .with_registry(|registry| {
            crate::movement::resolve_spawn(&registry.arena.view(&server.lobby), requested)
        })
        .unwrap_or(Transform { x, y, z, ..requested });
    // Bots join in bursts; wait out a full registration queue like a
    // reconnecting client rather than giving up.
    let session = loop {
        match lobby.register(profile.clone(), spawn, 0x7f).await {
            Err(crate::lobby::AdmissionError::QueueFull) => {
                Timer::new(TICK).await;
            }
            result => break result?,
        }
    };
    // Becoming visible follows the human path: teleport, then confirmation.
    session.teleport(spawn);

    let mut brain = Brain::new(skill, rng.next(), spawn);
    let mut body = Body::default();
    let mut decided_at = None;
    loop {
        // Decide once per simulation step, right after the owner publishes
        // it, so every observation is fresh and every input lands on the next
        // step. The timer covers waits before the bot becomes active.
        futures_lite::future::or(session.wait_changed(), async {
            Timer::new(TICK).await;
        })
        .await;
        body.sync(&session);
        if !session.ready() {
            continue;
        }
        let position = session.accepted_transform();
        let input = lobby.with_registry(|registry| {
            let step = registry.arena.tick();
            if decided_at.replace(step) == Some(step) {
                return None;
            }
            let mut nav = nav.borrow_mut();
            nav.sync(&registry.arena);
            Some(brain.tick(position, &mut nav, &registry.arena.view(&server.lobby)))
        });
        let Some(input) = input else { continue };
        body.drive(&session, input);
        let reset = brain.take_reset(&nav.borrow());
        if let Some(reset) = reset {
            // Stand-in for the course returning fallen players to their
            // checkpoint; the lobby has no such rule for humans yet.
            let to = reset.map_or(spawn, |cell| Transform {
                x: f64::from(cell[0]) + 0.5,
                y: f64::from(cell[1]),
                z: f64::from(cell[2]) + 0.5,
                ..spawn
            });
            session.teleport(to);
        }
        report.borrow_mut().bots[index] =
            (brain.mode(), position.y, brain.stats.failures, brain.checkpoint());
    }
}

/// Mirrors what a client does with server updates: confirm teleports and
/// press respawn after dying.
#[derive(Default)]
struct Body {
    dead_for: Option<u32>,
    driven: Option<MovementInput>,
}

impl Body {
    fn sync(&mut self, session: &Session) {
        let update = session.take_update();
        if let Some(teleport) = update.teleport {
            session.confirm_teleport(teleport.id);
        }
        if update.teleport.is_some() || update.respawn || update.death.is_some() {
            // Queued drives are discarded across these; resubmit once ready.
            self.driven = None;
        }
        if update.death.is_some() {
            self.dead_for = Some(0);
        }
        if let Some(ticks) = &mut self.dead_for {
            *ticks += 1;
            if *ticks >= RESPAWN_DELAY {
                session.respawn();
                self.dead_for = None;
            }
        }
    }

    fn drive(&mut self, session: &Session, input: MovementInput) {
        let unchanged = self.driven.is_some_and(|driven| same_input(driven, input));
        if !unchanged && session.drive(Some(input)) {
            self.driven = Some(input);
        }
    }
}

fn same_input(a: MovementInput, b: MovementInput) -> bool {
    (a.forward, a.backward, a.left, a.right, a.jumping, a.sneaking, a.sprinting)
        == (b.forward, b.backward, b.left, b.right, b.jumping, b.sneaking, b.sprinting)
        && a.yaw == b.yaw
        && a.pitch == b.pitch
}

#[cfg(test)]
#[path = "../tests/tests_bot.rs"]
mod tests;
