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
/// Route validation time shared by all bots per server tick.
const NAV_BUDGET: Duration = Duration::from_millis(2);
/// How far from spawn the climb goal is searched for.
const GOAL_RADIUS: i32 = 128;

/// Live per-bot summaries for diagnostics.
#[derive(Default)]
struct Report {
    bots: Vec<(Mode, f64, u32)>,
}

/// Spawn `count` bots on the current executor. They share one navigation
/// service, so routes validated for one bot are reused by all.
pub fn spawn(server: Arc<Server>, lobby: Lobby, count: usize, seed: u64) {
    if count == 0 {
        return;
    }
    let mut service = NavService::new(Budget::PerWindow { limit: NAV_BUDGET, window: TICK });
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
    let nav = Rc::new(RefCell::new(service));
    let report = Rc::new(RefCell::new(Report { bots: vec![(Mode::Idle, 0.0, 0); count] }));
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
        let count = |mode| report.bots.iter().filter(|(m, _, _)| *m == mode).count();
        let mut heights: Vec<f64> = report.bots.iter().map(|(_, y, _)| *y).collect();
        heights.sort_by(f64::total_cmp);
        let falls: u32 = report.bots.iter().map(|(_, _, falls)| falls).sum();
        eprintln!(
            "bots: idle={} planning={} moving={} recovering={} finished={} stuck={} falls={} y_median={:.1} y_max={:.1} nav_nodes={} nav_edges={} nav_cells={} sims={} sim_ticks={} build_ms={} expansions={}",
            count(Mode::Idle),
            count(Mode::Planning),
            count(Mode::Moving),
            count(Mode::Recovering),
            count(Mode::Finished),
            count(Mode::Stuck),
            falls,
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
    let mut next = Instant::now();
    loop {
        next += TICK;
        Timer::new(next.saturating_duration_since(Instant::now())).await;
        body.sync(&session);
        if !session.ready() {
            continue;
        }
        let position = session.accepted_transform();
        let input = lobby.with_registry(|registry| {
            let mut nav = nav.borrow_mut();
            nav.sync(&registry.arena);
            brain.tick(position, &mut nav, &registry.arena.view(&server.lobby))
        });
        body.drive(&session, input);
        report.borrow_mut().bots[index] = (brain.mode(), position.y, brain.stats.falls);
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
