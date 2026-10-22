//! Server-driven participants. A bot owns a lobby `Session` like a human
//! connection and acts only through the shared action path.

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

const TICK: Duration = Duration::from_millis(50);
/// Ticks a dead bot waits before pressing respawn.
const RESPAWN_DELAY: u32 = 30;

/// Spawn `count` wandering bots on the current executor.
pub fn spawn(server: Arc<Server>, lobby: Lobby, count: usize, seed: u64) {
    for index in 0..count {
        let server = server.clone();
        let lobby = lobby.clone();
        let seed = seed ^ (index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        glommio::spawn_local(async move {
            if let Err(error) = run(server, lobby, index, seed).await {
                eprintln!("bot {index}: {error}");
            }
        })
        .detach();
    }
}

async fn run(
    server: Arc<Server>,
    lobby: Lobby,
    index: usize,
    seed: u64,
) -> Result<(), crate::error::Error> {
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

    let mut brain = Wander::new(spawn.yaw);
    let mut body = Body::default();
    let mut next = Instant::now();
    loop {
        next += TICK;
        Timer::new(next.saturating_duration_since(Instant::now())).await;
        body.sync(&session);
        if !session.ready() {
            continue;
        }
        let input = brain.tick(&mut rng, session.accepted_transform());
        body.drive(&session, input);
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

/// Placeholder behavior until navigation exists: alternate between idling
/// with a wandering gaze and walking toward a random heading.
struct Wander {
    walking: bool,
    ticks_left: u32,
    yaw: f32,
    pitch: f32,
    target_yaw: f32,
    target_pitch: f32,
    last: Option<Transform>,
    blocked: u32,
}

impl Wander {
    fn new(yaw: f32) -> Self {
        Self {
            walking: false,
            ticks_left: 20,
            yaw,
            pitch: 0.0,
            target_yaw: yaw,
            target_pitch: 0.0,
            last: None,
            blocked: 0,
        }
    }

    fn tick(&mut self, rng: &mut Rng, position: Transform) -> MovementInput {
        let moved = self
            .last
            .replace(position)
            .is_some_and(|last| (last.x - position.x).hypot(last.z - position.z) > 0.02);
        self.blocked = if self.walking && !moved { self.blocked + 1 } else { 0 };

        if self.ticks_left == 0 || self.blocked > 10 {
            self.walking = !self.walking || self.blocked > 10;
            self.blocked = 0;
            if self.walking {
                self.ticks_left = rng.range(30.0, 120.0) as u32;
                self.target_yaw = self.yaw + rng.range(-120.0, 120.0) as f32;
                self.target_pitch = rng.range(-5.0, 15.0) as f32;
            } else {
                self.ticks_left = rng.range(20.0, 80.0) as u32;
            }
        }
        self.ticks_left -= 1;
        if !self.walking && rng.chance(0.04) {
            self.target_yaw = self.yaw + rng.range(-70.0, 70.0) as f32;
            self.target_pitch = rng.range(-20.0, 25.0) as f32;
        }

        // Eased turn with a rate cap and a little tremor, never a snap.
        let turn = (self.target_yaw - self.yaw) * 0.25;
        self.yaw += turn.clamp(-18.0, 18.0) + rng.range(-0.3, 0.3) as f32;
        self.pitch += (self.target_pitch - self.pitch) * 0.2 + rng.range(-0.2, 0.2) as f32;
        self.pitch = self.pitch.clamp(-90.0, 90.0);
        let wrap = (self.yaw / 360.0).round() * 360.0;
        self.yaw -= wrap;
        self.target_yaw -= wrap;

        MovementInput {
            forward: self.walking,
            // Try hopping over an obstacle before giving up on the heading.
            jumping: self.walking && self.blocked > 3,
            yaw: self.yaw,
            pitch: self.pitch,
            ..Default::default()
        }
    }
}

/// SplitMix64: tiny, seedable and reproducible across runs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.unit()
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

#[cfg(test)]
#[path = "../tests/tests_bot.rs"]
mod tests;
