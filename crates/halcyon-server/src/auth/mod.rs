use crate::server::Server;

use mc_protocol::auth::Response;
use mc_protocol::data::motd::Player;

use std::sync::Arc;

const BASE_URL: &'static str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";

// NOTE: Vanilla server supports an optional ip component

async fn authenticate(player: &Player<'_>, server: Arc<Server>) {
    let username = player.name;
    let hash = String::new();

    let url = format!("?username={username}&serverId={hash}");
    let http_client = reqwest::Client::new();

    let res = http_client.get(url).send().await.unwrap();
    let res = res.error_for_status().unwrap();
    let v: Response = res.json().await.unwrap();
}
