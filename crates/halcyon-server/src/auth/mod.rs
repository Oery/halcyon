use mc_protocol::auth::Response;
use std::{sync::OnceLock, time::Duration};

const BASE_URL: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";
static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

// NOTE: Vanilla server supports an optional ip component
// when `prevent-proxy-connections` is enabled; we omit it.
pub async fn authenticate(username: &str, hash: &str) -> Result<Response, String> {
    let username = username.to_owned();
    let hash = hash.to_owned();

    // NOTE: `ureq` is blocking with no runtime, so the whole request runs
    // inside `spawn_blocking`, off the latency-sensitive executor thread.
    glommio::executor()
        .spawn_blocking(move || {
            AGENT
                .get_or_init(|| {
                    ureq::Agent::config_builder()
                        .timeout_global(Some(Duration::from_secs(10)))
                        .build()
                        .new_agent()
                })
                .get(format!("{BASE_URL}?username={username}&serverId={hash}"))
                .call()
                .map_err(|e| e.to_string())?
                .body_mut()
                .read_json::<Response>()
                .map_err(|e| e.to_string())
        })
        .await
}
