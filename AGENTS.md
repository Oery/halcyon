# Repository Guidelines

## Project Vision

Halcyon is a Minecraft Java-edition minigame server built from scratch in Rust. Players should be able to enjoy a variety of minigames even when few human players are online, with humanlike bots filling games and providing engaging opponents and teammates.

Supporting very large bot populations is a core design requirement. The server must be highly efficient and scale vertically, making effective use of the CPU cores and memory available on a single machine.

The target authoritative simulation rate is **120 TPS for worlds with up to 100 players**, counting humans and bots together, giving each simulation step an approximately **8.33 ms** deadline. This is a per-world performance target; the server may host multiple worlds and a larger total population. Preserve supported vanilla mechanics and their real-time timing; a higher scheduling rate must not make gameplay run six times faster. Network publication, bot decisions, and maintenance may have independent cadences. Budget for future gameplay systems and measure tail latency and missed deadlines, rather than spending the full deadline on basic player replication. Extremely crowded lobbies need a separately measured service budget; maintaining 120 TPS with 1,000 players in one lobby is not a current requirement. The existing 50 ms lobby publication loop is the current implementation, not the target simulation clock.

Halcyon does not aim for full feature parity with the official Minecraft server. Implement only the Minecraft features required by Halcyon's minigames, bots, and player experience. Design each feature around those needs rather than reproducing vanilla internals.

Implementation principles:

- Evaluate designs against their CPU, memory, and scheduling costs as player and bot populations grow.
- Prefer shared immutable data, reuse, batching, and bounded work where appropriate; substantiate performance claims with measurements.
- Preserve required client protocol compatibility while tailoring server behavior to Halcyon's needs.
- Treat the documented architecture as the current implementation, not a permanent constraint on vertical scaling.

## Current Implementation

Early-stage Minecraft Java-edition server in Rust. TCP accept on `127.0.0.1:25565` → per-connection packet loop. Implements status, authenticated encrypted login, configuration, and a shared multiplayer lobby with a read-only world and chunk streaming. Players have unique entity IDs, lobby-wide Tab profiles, terrain-gated entity visibility, and coalesced movement/skin updates. Registry/block-state data targets 26.3. No authoritative collision/physics, bots, packet compression, lighting engine, or mutable world simulation yet.

## Protocol Reference

- Source of truth: `https://minecraft.wiki/w/Java_Edition_protocol/Packets` — authoritative for packet IDs, states, field order/types.
- Content is very large: NEVER dump full page; always trim after fetch to the exact packet/state section needed.

## Architecture & Data Flow

Flow: `TcpListener.incoming()` → `Client{stream, ctx, server: Arc<Server>}` → bounded receive buffer → `read_packet(buf, &ctx)` → `Packets::read` matched on `(packet_id, ctx.state)` → state handler. Dispatch uses the connection state because payload types can be shared across states. Dynamic packets encode into a reusable buffer; cached configuration frames and chunk parts bypass repeated encoding. Ciphertext persists across partial writes and `Pending`, and sends flush explicitly.

Crate dependency: `halcyon-server → mc_protocol → macros` (proc-macro), plus `halcyon-server → halcyon-world`. Protocol owns codecs; macros owns `#[packet]` codegen; server owns I/O, lifecycle, wire caches and chunk scheduling; world owns disk parsing and immutable pre-encoded chunk sections.

An executor-local `Lobby` owns shared human-player state and publishes latest input every 50 ms; `Arc<Server>` retains immutable startup resources. Each `Client` holds a unique session guard and an `Observer` that tracks delivered profiles/entities and bounded output. Registry borrows end before socket I/O. Dropping the guard signals cleanup without needing registration queue space. Independent lobby ownership is the boundary for future executor placement; the runtime still uses one core.

Add a packet: define `#[packet(State, 0xNN, Client|Server)] struct` in `crates/mc-protocol/src/packets/{server,client}.rs` → add `Packets` enum variant → add `(id, state)` arm in `Packets::read` → handle in `crates/halcyon-server/src/states/<state>.rs`.

## Key Directories

- `crates/halcyon-server/src/`: runtime — `main.rs`, `server.rs`, `client.rs`, `transport.rs`, `packet.rs`, `lifecycle.rs`, `lobby.rs`, `visibility.rs`, `cache.rs`, `chunks.rs`, `states/`, `auth/`, `encryption/`.
- `crates/halcyon-world/src/`: startup world loading — `loader.rs`, `disk_nbt.rs`, `blockstates.rs`, immutable world grid in `lib.rs`.
- `crates/mc-protocol/src/`: codec — `encode.rs`, `decode.rs`, `error.rs`, `state.rs`, `packets/{mod,client,server}.rs`, `types/{varint,int,str,bytes,uuid,game_profile}.rs`, `data/{motd,config/*}`, `auth/`.
- `crates/mc-protocol-macros/src/`: `lib.rs`, `attrs.rs`, `format.rs` — `#[packet]` / `Payload` / `InnerPacket` codegen.
- `docs/`: vanilla feature inventory and Halcyon feature assessment. No `scripts/`, `tools/`, standalone `tests/`, `benches/`, `examples/`, `.github/`, or `Dockerfile`.

## Development Commands

Stock Cargo, no wrappers/scripts:

```sh
cargo run -p halcyon-server          # binary `halcyon` -> target/debug/halcyon
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The macro crate is an automatic workspace member through its path dependency; workspace checks include it (`cargo metadata` confirms this).

## Code Conventions & Common Patterns

- Formatting: `rustfmt.toml` (`single_line_if_else_max_width=60`, `single_line_let_else_max_width=60`, `use_small_heuristics="Max"`). Run `cargo fmt` before yielding.
- Codec traits: `PacketWriter{read, write, body_len}`, `Payload{const ID, const STATE, payload_len, decode_payload, write_payload, write_packet}`, `InnerPacket{id(), state()}`. Zero-copy decode borrows `&'p str` / `&[u8]` from read buffer.
- Packet declaration: `#[packet(Login, 0x00, Client)]` + field `#[format="varint"|"json"]`. Direction files: `packets/server.rs` = client→server, `packets/client.rs` = server→client.
- Naming: packets `PascalCase` + `*Request/*Response` (`ServerListPing`, `LoginStart`, `EncryptionRequest`); methods `handle_packet/read_packet/write_packet/payload_len/decode_payload/write_payload`; fields `snake_case` (`verify_token`, `next_state`); consts `UPPER_SNAKE` (`ADDR`, `SIZE`); traits `PacketWriter/Payload/InnerPacket`.
- State: `Context` holds wire state and profile data. Handshake decodes `HandshakeIntent`, never arbitrary `State`. `LoginProgress` guards authentication and acknowledgement. Handlers own transitions through Login → Config → Play. `compression` is unused; do not negotiate it until framing supports it.
- Async: `glommio` `Fixed(0)` single-place executor, one local task per connection. Receive storage starts at 1024 bytes and grows to at most `MAX_FRAME_LEN + 3`. A complete frame with an incomplete payload is invalid, not a partial network read. Parse loops and chunk sends yield explicitly; blocking HTTPS runs off the executor with a request timeout.
- Scheduling: login/config deadline 60 s, writes 10 s, Play keepalive every 15 s with a 30 s response deadline. Chunk batches wait for acknowledgement, respect bounded client rate feedback, and cap view distance at 8. Residency distinguishes real terrain and empty padding. Keep configuration/chunk encodings shared; lighting suffix is shared separately to avoid duplicating 104 KiB per cached chunk.
- Multiplayer: at most 1,024 reserved/active humans and 256 pending registrations; this is an admission bound, not a measured network capacity. Initial teleport confirmation has a 30 s deadline. Latest movement/settings replace pending input, and missed publication ticks are skipped. Observer output is bounded to 256 KiB/2,048 protocol frames, with 16 player admissions per publication and drain slices of 32 frames/64 KiB. Entity removals precede terrain unload/replacement; empty padding never enables entity spawns. Tab profiles persist throughout lobby membership. Relative movement uses Java-compatible `Math.round(position * 4096)` deltas against delivered positions.
- DI: `Arc<Server>` cloned into `Client::new(stream, server)`; handlers reach keys via `client.server.keys`.
- Error handling: typed decode/encode/connection errors; malformed client input ends only that connection. Validate lengths before allocation. Unknown packets consume their frame without logging. Set `HALCYON_DIAGNOSTICS=1` for connection failures and five-second aggregate lobby/observer metrics; avoid unconditional per-packet or per-chunk output.

## Important Files

- `crates/halcyon-server/src/main.rs`: `ADDR`, `MiMalloc` global allocator, `glommio::main`, accept loop.
- `crates/halcyon-server/src/client.rs`, `packet.rs`: read loop, VarInt framing.
- `crates/halcyon-server/src/states/mod.rs`, `login.rs`, `status.rs`: connection-state dispatch, authenticated login, status response with actual active lobby population (`protocol: 777`).
- `crates/halcyon-server/src/encryption/mod.rs`: `Keys`, `Rsa::generate(2048)`, `generate_verify_token`, `decrypt_rsa` (PKCS1).
- `crates/mc-protocol/src/packets/mod.rs`, `state.rs`, `error.rs`, `encode.rs`, `decode.rs`: `Context`, `State`, traits, error enums.
- `crates/mc-protocol-macros/src/lib.rs`, `attrs.rs`, `format.rs`: codegen entry points.
- `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `.gitignore`, `crates/*/Cargo.toml`.

## Runtime/Tooling Preferences

- Runtime: Rust `edition = "2024"`, `resolver = "3"`, no pinned toolchain/MSRV — ambient stable. Package manager: Cargo, `Cargo.lock` committed.
- Linux-only (`glommio-ng 0.12` io_uring); build needs system OpenSSL headers (`openssl 0.10`).
- No Node/Bun/Python/Docker/CI. Do not add JS tooling.
- `.gitignore` is deny-by-default (`*` + `!*/`), with explicit exceptions for Rust, manifests, root configuration, and this file. New documentation, scripts, fixtures or CI files need whitelist entries.
- The macro crate is automatically included in the workspace despite not appearing in the root `members` list.

## Testing & QA

- Minimal tests: the repo should not be lost in a test soup. tests should only be added when required, for small units. They should not verify the entire server working, only small chunks.
- Live-client verification: `prismlauncher -l <instance> -s 127.0.0.1:25565` is allowed for test runs; choose an instance matching the current packet/registry version. Always close the client and the server afterward.
- QA = `cargo fmt --check` + `cargo clippy --workspace --all-targets`.
- Opt-in lobby measurement: `cargo test -p halcyon-server --release synthetic_lobby -- --ignored --nocapture`. Uses synthetic player records and in-memory observer sinks; reports publication/fanout timings, encoded bytes, queue bounds and payload-capacity estimates. It excludes sockets, encryption and chunk streaming and does not establish a supported population.
