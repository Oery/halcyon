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
use mc_protocol::packets::client::{InteractionHand, PlayerInfoUpdate};
use uuid::Uuid;

use crate::game::{Destination, Hub};
use crate::instance::{
    Instance, InstanceId, InstanceManager, Profile, Registry, Session, Transform,
};
use crate::movement::MovementInput;
use crate::server::Server;

pub mod brain;
pub mod fighter;
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
use fighter::{Fighter, Opponent, Senses};
use nav::{Budget, NavService};
use profile::BotProfile;
use rng::Rng;

const TICK: Duration = Duration::from_millis(50);
/// Ticks a dead bot waits before pressing respawn.
const RESPAWN_DELAY: u32 = 30;
/// Route validation time shared by all bots per server tick, unless
/// `HALCYON_BOT_NAV_MS` overrides it. Searches pause when it is spent.
const NAV_BUDGET: Duration = Duration::from_millis(8);
/// How far from spawn the climb goal is searched for.
const GOAL_RADIUS: i32 = 128;
/// Ticks a queueing bot spends in a hub before asking for a game.
const QUEUE_WAIT: [u32; 2] = [60, 300];
/// The chat line a round's start is announced with.
const FIGHT: &str = crate::game::FIGHT;

/// Live per-bot summaries for diagnostics.
#[derive(Default)]
struct Report {
    bots: Vec<(Mode, f64, [u32; 3], usize)>,
    /// Ticks stalled and moves explored, summed over bots.
    stalled: u64,
    explored: u64,
    /// Searches finished since the last report, and the ticks they took.
    searches: u64,
    search_ticks: u64,
}

/// Spawn `count` bots on the current executor. They share one navigation
/// service, so routes validated for one bot are reused by all.
pub fn spawn(server: Arc<Server>, instances: InstanceManager, count: usize, seed: u64) {
    if count == 0 {
        return;
    }
    let Some(lobby) = instances.home() else { return };
    // HALCYON_BOT_GAME=<kind>: bots queue for that game from the hub, over
    // and over, instead of climbing the hub's course.
    let queue = std::env::var("HALCYON_BOT_GAME")
        .ok()
        .and_then(|name| crate::game::kind(&name))
        .filter(|kind| *kind != Hub::KIND);
    if let Some(kind) = queue {
        println!("bots: queueing for {kind}");
    }
    let limit = std::env::var("HALCYON_BOT_NAV_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(NAV_BUDGET, Duration::from_millis);
    let mut service = NavService::new(Budget::PerWindow { limit, window: TICK });
    service.goal = std::env::var("HALCYON_BOT_GOAL").ok().and_then(|goal| parse_cell(&goal));
    if service.goal.is_none() {
        let (x, y, z, _, _) = lobby.world().spawn;
        let around = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        let start = Instant::now();
        service.goal = lobby.with_registry(|registry| {
            service.detect_goal(&registry.arena.view(lobby.world()), around, GOAL_RADIUS)
        });
        println!("bots: goal {:?} detected in {}ms", service.goal, start.elapsed().as_millis());
    }
    if std::env::var_os("HALCYON_BOT_NO_CHECKPOINTS").is_none() {
        let (x, y, z, _, _) = lobby.world().spawn;
        let around = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        let start = Instant::now();
        service.checkpoints = lobby.with_registry(|registry| {
            service.detect_checkpoints(&registry.arena.view(lobby.world()), around, GOAL_RADIUS)
        });
        println!(
            "bots: {} checkpoints detected in {}ms",
            service.checkpoints.len(),
            start.elapsed().as_millis()
        );
    }
    // HALCYON_BOT_CHECKPOINT=n: start on the n-th checkpoint (from 1), for
    // working on later parts of a course without waiting for the climb.
    let start = std::env::var("HALCYON_BOT_CHECKPOINT")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
        .map(|n| n.min(service.checkpoints.len()))
        .and_then(|n| Some((n, *service.checkpoints.get(n.checked_sub(1)?)?)));
    if let Some((n, cell)) = start {
        println!("bots: starting on checkpoint {n} at {cell:?}");
    }
    let nav = Rc::new(RefCell::new(service));
    let report = Rc::new(RefCell::new(Report {
        bots: vec![(Mode::Idle, 0.0, [0; 3], 0); count],
        stalled: 0,
        explored: 0,
        searches: 0,
        search_ticks: 0,
    }));
    let (low, high) = profile::parse_levels(std::env::var("HALCYON_BOT_LEVEL").ok().as_deref());
    for index in 0..count {
        let lobby = lobby.clone();
        let instances = instances.clone();
        let nav = nav.clone();
        let report = report.clone();
        let seed = seed ^ (index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let level = Rng::new(seed ^ 0x1e7e1).ticks(u32::from(low), u32::from(high)) as u8;
        glommio::spawn_local(async move {
            let home = lobby.id();
            let profile = BotProfile::new(level);
            let bot = Bot { index, seed, profile, start, nav, report, instances, home, queue };
            if let Err(error) = run(lobby, bot).await {
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
        let mut report = report.borrow_mut();
        let stats = nav.borrow().stats();
        let searches = std::mem::take(&mut report.searches);
        let search_ticks = std::mem::take(&mut report.search_ticks);
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
            "bots: idle={} exploring={} moving={} swimming={} finished={} fighting={} stalled={} explored={} searches={} search_mean_s={:.1} fell={} missed={} timeouts={} checkpoint_median={} checkpoint_max={}/{} y_median={:.1} y_max={:.1} nav_nodes={} nav_edges={} nav_cells={} sims={} sim_ticks={} build_ms={} expansions={}",
            count(Mode::Idle),
            count(Mode::Exploring),
            count(Mode::Moving),
            count(Mode::Swimming),
            count(Mode::Finished),
            count(Mode::Fighting),
            report.stalled,
            report.explored,
            searches,
            search_ticks as f64 / searches.max(1) as f64 / 20.0,
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
    /// The checkpoint to start on, by number, and its cell.
    start: Option<(usize, nav::Cell)>,
    nav: Rc<RefCell<NavService>>,
    report: Rc<RefCell<Report>>,
    instances: InstanceManager,
    /// The hub whose course `nav` describes.
    home: InstanceId,
    /// The game kind this bot queues for from a hub, if any.
    queue: Option<&'static str>,
}

/// What a bot does in its current instance.
enum Mind {
    /// Climbing the home hub's course.
    Climber(Box<Brain>),
    Fighter(Box<Fighter>),
}

async fn run(home: Instance, bot: Bot) -> Result<(), crate::error::Error> {
    let mut rng = Rng::new(bot.seed);
    let username = format!("Bot{}", bot.index + 1);
    let uuid = Uuid::from_u64_pair(rng.next(), rng.next());
    let mut info = Vec::new();
    PlayerInfoUpdate { uuid, username: &username, properties: vec![] }
        .write_packet(&mut info)
        .await?;
    let profile = Rc::new(Profile { uuid, username, info: Rc::from(info) });

    let (x, y, z, _, _) = home.world().spawn;
    // Spread around spawn, or within a starting checkpoint's plate.
    let ([x, y, z], spread) = match bot.start {
        Some((_, [cx, cy, cz])) => {
            ([f64::from(cx) + 0.5, f64::from(cy), f64::from(cz) + 0.5], 0.25)
        }
        None => ([x, y, z], 3.0),
    };
    let requested = Transform {
        x: x + rng.range(-spread, spread),
        y,
        z: z + rng.range(-spread, spread),
        yaw: rng.range(-180.0, 180.0) as f32,
        pitch: 0.0,
        on_ground: false,
    };
    let spawn = home
        .with_registry(|registry| {
            crate::movement::resolve_spawn(&registry.arena.view(home.world()), requested)
        })
        .unwrap_or(Transform { x, y, z, ..requested });

    let mut instance = home;
    let mut entity_id = None;
    loop {
        // Bots join in bursts; wait out a full registration queue like a
        // reconnecting client rather than giving up.
        let session = loop {
            match instance.register(entity_id, profile.clone(), spawn, 0x7f).await {
                Err(crate::instance::AdmissionError::QueueFull) => {
                    Timer::new(TICK).await;
                }
                result => break result?,
            }
        };
        entity_id = Some(session.entity_id());
        let destination = stay(&instance, &session, &bot, spawn, &mut rng).await;
        // The old owner removes the player on its next pass.
        drop(session);
        let next = bot.instances.resolve(destination).or_else(|| bot.instances.hub());
        let Some(next) = next else {
            return Err(crate::instance::AdmissionError::Closed.into());
        };
        instance = next;
    }
}

/// Plays in `instance` until the game, or the bot's own queueing, sends it
/// elsewhere.
async fn stay(
    instance: &Instance,
    session: &Session,
    bot: &Bot,
    spawn: Transform,
    rng: &mut Rng,
) -> Destination {
    let in_home = instance.id() == bot.home;
    // Becoming visible follows the human path: teleport, then confirmation.
    // The home hub keeps each bot's own spread; games place their players.
    let placed = if in_home { spawn } else { session.accepted_transform() };
    session.teleport(placed);

    let mut mind = if in_home && bot.queue.is_none() {
        let mut brain = Brain::new(bot.profile, rng.next(), placed);
        if let Some((n, _)) = bot.start {
            brain.start_at(n);
        }
        let name = format!("Bot{}", bot.index + 1);
        if std::env::var("HALCYON_BOT_TRACE").is_ok_and(|traced| traced == name || traced == "all")
        {
            brain.traced = Some(name);
        }
        Mind::Climber(Box::new(brain))
    } else {
        Mind::Fighter(Box::new(Fighter::new(bot.profile, rng.next(), placed)))
    };
    // Queueing bots wait a while in a hub before asking for a game, as a
    // player chatting in the lobby would.
    let mut queue_in = bot
        .queue
        .filter(|_| instance.kind() == Hub::KIND)
        .map(|kind| (kind, rng.ticks(QUEUE_WAIT[0], QUEUE_WAIT[1])));
    let mut body = Body::default();
    let mut decided_at = None;
    let mut seen = brain::BrainStats::default();
    // Hurt since the last decision, and the opponents scratch list.
    let mut hurt = false;
    let mut opponents = Vec::new();
    loop {
        // Decide once per simulation step, right after the owner publishes
        // it, so every observation is fresh and every input lands on the next
        // step. The timer covers waits before the bot becomes active.
        futures_lite::future::or(session.wait_changed(), async {
            Timer::new(TICK).await;
        })
        .await;
        let heard = body.sync(session);
        hurt |= heard.hurt;
        if let Some(destination) = heard.destination {
            let here = match destination {
                Destination::Hub => Some(Hub::KIND),
                Destination::Matchmake(kind) => Some(kind),
                Destination::Instance(_) => None,
            };
            if here != Some(instance.kind()) {
                return destination;
            }
        }
        if let Mind::Fighter(fighter) = &mut mind {
            // A death and the next fight's start can arrive in one update.
            if heard.died {
                fighter.engaged = false;
            }
            if heard.fight {
                fighter.engaged = true;
            }
        }
        if !session.ready() {
            continue;
        }
        if let Some((kind, ticks)) = &mut queue_in {
            *ticks = ticks.saturating_sub(1);
            if *ticks == 0 {
                return Destination::Matchmake(kind);
            }
        }
        let position = session.accepted_transform();
        let me = session.entity_id();
        let decided = instance.with_registry(|registry| {
            let step = registry.arena.tick();
            if decided_at.replace(step) == Some(step) {
                return None;
            }
            let view = registry.arena.view(instance.world());
            Some(match &mut mind {
                Mind::Climber(brain) => {
                    let mut nav = bot.nav.borrow_mut();
                    nav.sync(&registry.arena);
                    (brain.tick(position, &mut nav, &view), None)
                }
                Mind::Fighter(fighter) => {
                    let senses = perceive(registry, me, position, hurt, &mut opponents);
                    let decision = fighter.tick(&senses, &view);
                    (decision.input, Some(decision))
                }
            })
        });
        let Some((input, fought)) = decided else { continue };
        hurt = false;
        match fought {
            Some(decision) => act(session, &mut body, &decision),
            None => body.drive(session, input),
        }
        let Mind::Climber(brain) = &mut mind else {
            bot.report.borrow_mut().bots[bot.index] = (Mode::Fighting, position.y, [0; 3], 0);
            continue;
        };
        if let Some(pos) = brain.take_use() {
            // The same hand use a client sends; the owner checks reach.
            session.use_block(crate::interaction::BlockUse {
                face: 1,
                sequence: 0,
                pos,
                cursor: [0.5; 3],
                main_hand: true,
            });
        }
        let reset = brain.take_reset(&bot.nav.borrow());
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
        let mut report = bot.report.borrow_mut();
        report.bots[bot.index] =
            (brain.mode(), position.y, brain.stats.failures, brain.checkpoint());
        let stats = brain.stats;
        report.stalled += u64::from(stats.stalled - seen.stalled);
        report.explored += u64::from(stats.explored - seen.explored);
        report.searches += u64::from(stats.searches - seen.searches);
        report.search_ticks += u64::from(stats.search_ticks - seen.search_ticks);
        seen = stats;
    }
}

/// A game's own sparring bot: registers in `instance`, fights at the level
/// the game sets, and leaves when the game is done with it.
pub fn spar(instance: Instance, sparring: Rc<crate::game::Sparring>) {
    glommio::spawn_local(async move {
        if let Err(error) = run_sparring(instance, sparring).await {
            eprintln!("sparring bot: {error}");
        }
    })
    .detach();
}

async fn run_sparring(
    instance: Instance,
    sparring: Rc<crate::game::Sparring>,
) -> Result<(), crate::error::Error> {
    let id = instance.id();
    let mut rng = Rng::new(0x5ba1_0000 ^ (u64::from(id.shard) << 32) ^ u64::from(id.local));
    let uuid = Uuid::from_u64_pair(rng.next(), rng.next());
    let mut info = Vec::new();
    PlayerInfoUpdate { uuid, username: &sparring.name, properties: vec![] }
        .write_packet(&mut info)
        .await?;
    let profile = Rc::new(Profile { uuid, username: sparring.name.clone(), info: Rc::from(info) });
    let spawn = instance.map().spawn();
    let session = loop {
        match instance.register(None, profile.clone(), spawn, 0x7f).await {
            Err(crate::instance::AdmissionError::QueueFull) => {
                Timer::new(TICK).await;
            }
            result => break result?,
        }
    };
    session.teleport(session.accepted_transform());
    let new_fighter = |rng: &mut Rng| {
        let profile = BotProfile::new(sparring.level.get());
        Fighter::new(profile, rng.next(), session.accepted_transform())
    };
    let mut fighter = new_fighter(&mut rng);
    let mut generation = sparring.generation.get();
    let (mut body, mut hurt, mut opponents, mut decided_at) =
        (Body::default(), false, Vec::new(), None);
    while !sparring.closed.get() && !instance.closed() {
        futures_lite::future::or(session.wait_changed(), async {
            Timer::new(TICK).await;
        })
        .await;
        let heard = body.sync(&session);
        hurt |= heard.hurt;
        if sparring.generation.get() != generation {
            // A new opponent level: fresh hands and tactics for the next fight.
            generation = sparring.generation.get();
            fighter = new_fighter(&mut rng);
        }
        if heard.died {
            fighter.engaged = false;
        }
        if heard.fight {
            fighter.engaged = true;
        }
        if !session.ready() {
            continue;
        }
        let me = session.entity_id();
        let position = session.accepted_transform();
        let decision = instance.with_registry(|registry| {
            let step = registry.arena.tick();
            if decided_at.replace(step) == Some(step) {
                return None;
            }
            let senses = perceive(registry, me, position, hurt, &mut opponents);
            Some(fighter.tick(&senses, &registry.arena.view(instance.world())))
        });
        if let Some(decision) = decision {
            hurt = false;
            act(&session, &mut body, &decision);
        }
    }
    Ok(())
}

/// What a fighter perceives in the published `registry`: other visible,
/// living players with their hurt and swing cues, as a client renders them.
pub(crate) fn perceive<'a>(
    registry: &Registry,
    me: i32,
    position: Transform,
    hurt: bool,
    opponents: &'a mut Vec<Opponent>,
) -> Senses<'a> {
    opponents.clear();
    opponents.extend(
        registry
            .players()
            .filter(|p| {
                p.entity_id != me
                    && p.visible
                    && p.health > 0.0
                    && p.game_mode != mc_protocol::types::GameMode::Spectator
            })
            .map(|p| Opponent {
                id: p.entity_id,
                transform: p.transform,
                cues: registry.cues(p.entity_id),
            }),
    );
    let own = registry.get(me);
    let delay = own
        .and_then(|p| crate::combat::weapon(&p.equipment[0]))
        .map_or(5.0, crate::combat::Weapon::delay);
    let health = own.map_or(20.0, |p| p.health);
    Senses { tick: registry.arena.tick(), me: position, health, hurt, delay, opponents }
}

/// Sends a decision the way a client would: movement, then a swing with
/// every attack.
pub(crate) fn act(session: &Session, body: &mut Body, decision: &fighter::Decision) {
    body.drive(session, decision.input);
    if decision.swing {
        session.swing(InteractionHand::Main);
    }
    if let Some(target) = decision.attack {
        session.attack(target);
    }
}

/// What a bot noticed in its latest server updates.
#[derive(Default)]
pub(crate) struct Heard {
    pub destination: Option<Destination>,
    pub died: bool,
    /// It took damage.
    pub hurt: bool,
    /// The round announced its start.
    pub fight: bool,
}

/// Mirrors what a client does with server updates: confirm teleports and
/// press respawn after dying.
#[derive(Default)]
pub(crate) struct Body {
    dead_for: Option<u32>,
    driven: Option<MovementInput>,
}

impl Body {
    pub fn sync(&mut self, session: &Session) -> Heard {
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
        Heard {
            destination: update.destination,
            died: update.death.is_some(),
            hurt: update.damage.is_some() || update.death.is_some(),
            fight: update.messages.iter().any(|line| &**line == FIGHT),
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
