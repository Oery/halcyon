//! One executor per core. Each shard accepts connections (the kernel spreads
//! them with SO_REUSEPORT), runs its own hub and game instances, and owns
//! every connection whose player is in one of them. Gameplay never crosses
//! shards; only connection handoffs do.
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures_lite::stream::StreamExt;
use glommio::channels::channel_mesh::{Full, MeshBuilder, Senders};
use glommio::net::TcpListener;
use glommio::timer::Timer;

use crate::client::{Client, Parked};
use crate::instance::InstanceManager;
use crate::server::Server;

/// Handoffs waiting for one shard from each other shard.
pub const HANDOFF_CAPACITY: usize = 64;
const PUBLISH_INTERVAL: Duration = Duration::from_millis(250);

pub enum ShardMsg {
    /// A connection whose player left an instance here for one on the receiver.
    Handoff(Box<Parked>),
}

pub type Mesh = MeshBuilder<ShardMsg, Full>;

/// This executor's identity and its senders to every other shard.
pub struct Shard {
    pub id: u16,
    senders: Senders<ShardMsg>,
}

impl Shard {
    /// Never waits: this shard, an unknown shard, or a full or closed one
    /// gives the message back.
    pub fn send(&self, to: u16, msg: ShardMsg) -> Result<(), ShardMsg> {
        use glommio::{GlommioError, ResourceType};
        if to == self.id || usize::from(to) >= self.senders.nr_consumers() {
            return Err(msg);
        }
        match self.senders.try_send_to(usize::from(to), msg) {
            Ok(()) => Ok(()),
            Err(
                GlommioError::Closed(ResourceType::Channel(msg))
                | GlommioError::WouldBlock(ResourceType::Channel(msg)),
            ) => Err(msg),
            // A checked peer index fails only with the message attached.
            Err(error) => unreachable!("shard {} -> {to}: {error}", self.id),
        }
    }
}

pub fn count() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    std::env::var("HALCYON_SHARDS")
        .ok()
        .and_then(|shards| shards.parse().ok())
        .unwrap_or(cores)
        .clamp(1, cores)
}

pub async fn run(server: Arc<Server>, mesh: Mesh, address: &'static str) {
    let Ok((senders, mut receivers)) = mesh.join().await else {
        eprintln!("shard: could not join the shard mesh");
        return;
    };
    let id = senders.peer_id() as u16;
    let shard = Rc::new(Shard { id, senders });
    let instances = InstanceManager::new(server.clone(), id);
    let Some(hub) = instances.home() else {
        eprintln!("shard {id}: the hub map is missing; set HALCYON_HUB_MAP");
        return;
    };

    if id == 0 {
        // Temporary until matches own bot populations: HALCYON_BOTS=<count>.
        let bots = std::env::var("HALCYON_BOTS").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
        crate::bot::spawn(server.clone(), hub, bots, 0x0068_616c_6379_6f6e);
    }

    for (_, receiver) in receivers.streams() {
        let (server, shard, instances) = (server.clone(), shard.clone(), instances.clone());
        glommio::spawn_local(async move {
            while let Some(msg) = receiver.recv().await {
                match msg {
                    ShardMsg::Handoff(parked) => {
                        let client = Client::resume(*parked, &server, &shard, &instances);
                        glommio::spawn_local(client).detach();
                    }
                }
            }
        })
        .detach();
    }

    let publisher = instances.clone();
    glommio::spawn_local(async move {
        loop {
            publisher.publish();
            Timer::new(PUBLISH_INTERVAL).await;
        }
    })
    .detach();

    let listener = match TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("shard {id}: bind {address} failed: {error}");
            return;
        }
    };
    if id == 0 {
        println!("Listening on {address} with {} shards", server.directory.shards());
    }
    let mut incoming = listener.incoming();
    while let Some(conn) = incoming.next().await {
        let conn = match conn {
            Ok(conn) => conn,
            Err(error) => {
                eprintln!("shard {id}: accept failed: {error}");
                continue;
            }
        };
        let Some(hub) = instances.home() else {
            eprintln!("shard {id}: hub unavailable; dropping connection");
            continue;
        };
        let client = Client::new(conn, server.clone(), shard.clone(), instances.clone(), hub);
        glommio::spawn_local(client.run()).detach();
    }
}
