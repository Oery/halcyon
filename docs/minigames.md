# Minigame framework

How minigames run on Halcyon, how to write one, and which game-facing
capabilities exist. Keep the tracker at the end current when a capability
changes; [feature-progress.md](feature-progress.md) keeps the server-wide view.

## Model

- **Map**: an immutable world folder in `worlds/`, loaded once at startup and
  shared by every instance that plays on it (`map.rs`).
- **Instance**: one running copy of a game. It owns a terrain overlay on its
  map, its players and a `Game`, and simulates at 20 Hz on one core
  (`instance/`). Players see only their own instance.
- **Game**: the minigame's rules, a type implementing `Game` (`game/`). The
  instance calls it synchronously inside its own step, so game state is plain
  owned data: no locks, no `async`.
- **Shard**: one executor per core (`shard.rs`). Each shard owns its instances
  and the connections of the players in them; a player moving to an instance
  on another shard takes its connection along.

## Lifecycle

```
/play <kind> ─► route: fullest open instance of <kind> on any shard,
                else a new one on the least-loaded shard (game::create)
                │
admission ────► on_join(ctx, player)   before the client learns its entity ID:
                                       place the player, set its rules
each 50 ms ───► simulation (movement, combat, blocks…)
                on_event(ctx, event)   Left, Died { player, killer }, Respawned
                tick(ctx)              the game's own state machine
                phase(), capacity()    read back for matchmaking
close() ──────► the instance stops once its last player has left
```

Matchmaking places players only in instances whose `phase()` is `Open` and
whose population (players still loading included) is below `capacity()`.

## Writing a game

1. Implement `Game` in `game/<name>.rs`:

   ```rust
   pub struct KingOfTheHill { /* state */ }

   impl KingOfTheHill {
       pub const KIND: &str = "koth";
   }

   impl Game for KingOfTheHill {
       fn kind(&self) -> &'static str { Self::KIND }
       fn capacity(&self) -> usize { 12 }
       fn phase(&self) -> Phase { Phase::Open }
       fn on_join(&mut self, ctx: &mut GameCtx<'_>, player: i32) {
           let spawn = ctx.map_spawn();
           ctx.teleport(player, spawn);
           ctx.set_rules(player, PlayerRules::lobby(spawn));
       }
       fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {}
       fn tick(&mut self, ctx: &mut GameCtx<'_>) {}
   }
   ```

2. Register it in `game/mod.rs`: add `KIND` to `KINDS` and an arm to
   `create` that picks its map (optionally through an environment override,
   like `HALCYON_FFA_MAP`).
3. `/play <kind>` now finds or starts it, with command completion.

To finish a round, send every player elsewhere and close:
`ctx.send_to(player, Destination::Hub)` for each player, then `ctx.close()`.
`game/ffa.rs` is the reference implementation.

## `GameCtx`

| Call | Effect |
| --- | --- |
| `players()`, `player_count()` | Admitted players in entity order, including those still loading. Allocation-free iterator; collect it to change players while looping. |
| `name(player)`, `position(player)`, `alive(player)` | Read a player's profile name, authoritative position and life. |
| `map_spawn()`, `instance()` | The map's spawn point and this instance's ID. |
| `teleport(player, transform)` | Nearest clear spot. Before the player is visible it sets the spawn; afterwards it is a queued teleport. |
| `set_rules(player, rules)` | Immediate `PlayerRules`: protection, fall/void damage, PvP, block editing, respawn point. |
| `reset(player)` | Queued full heal and fresh life at the rules' respawn point. |
| `message(player, text)`, `broadcast(text)` | System chat, at most 16 lines buffered per player. |
| `send_to(player, destination)` | Moves the connection to `Hub`, `Matchmake(kind)` or `Instance(id)`, on any shard. |
| `close()` | Stops the instance once it is empty. |

Queued calls (`reset`, `teleport` after spawning) share the player's 32-entry
action queue and apply on the next step.

## Built-in games

| Kind | Behavior |
| --- | --- |
| `hub` | Protected lobby (PvP with `HALCYON_PVP=1`). Holds `HALCYON_HUB_CAPACITY` players (default 100, bots included); players fill the fullest hub with room across shards. Each shard keeps one permanent hub; overflow hubs stop after 60 s empty. Map: `HALCYON_HUB_MAP`. |
| `ffa` | Free-for-all for 2–8 players: waits for two, 5 s countdown, PvP until one player is left, then everyone returns to the hub after 5 s. Abandoned instances close. Map: `HALCYON_FFA_MAP`. |

Both default to the `Parkour Spiral` map.

## Capability tracker

| Capability | Progress |
| --- | --- |
| Game definition and registration | Done — `Game` trait, `KINDS` and `create` in `game/mod.rs`. |
| Independent instances per game | Done — own terrain overlay, players, effects and tab list on a shared map. |
| Join hook with placement and rules | Done — `on_join` runs before the client receives its entity ID. |
| Death, respawn and departure events | Done — `Died` names a melee killer or damage source. |
| Per-step game logic | Done — `tick` at 20 Hz after simulation. |
| Matchmaking | Done — fullest open instance with room on any shard, else a new one on the least-loaded shard. |
| Moving players between games | Done — `send_to` and `/play`, `/hub`; across cores with the connection, keeping the entity ID. |
| Instance teardown | Done — `close()` stops an empty instance. |
| Map selection | Partial — one map per game kind through `create`; no rotation or voting. |
| Spawn assignment | Partial — `teleport` and `set_rules`; spawn points are computed in code. |
| Player rules | Partial — protection, fall/void damage, PvP and block editing per player; no hunger, natural regeneration or item drop rules. |
| Chat messages | Done — per-player and broadcast system chat. |
| Round timers | Partial — games count steps (`SIMULATION_HZ` per second); no scheduler. |
| Loadouts and item grants | Not started in `GameCtx` — the owner's inventory-edit path exists but is not exposed to games. |
| Game mode changes | Not started in `GameCtx` — e.g. spectator for eliminated players. |
| Terrain edits from games | Not started in `GameCtx` — `set_block`, signs, jukeboxes and openings exist on `Instance` only. |
| Titles, action bar and sounds | Not started |
| Scoreboards and sidebars | Not started |
| Boss bars | Not started |
| Teams | Not started — membership, nametag colors, friendly fire. |
| Objectives and regions | Not started — capture zones, checkpoints, region enter/leave events. |
| Map metadata | Not started — named spawn points and regions per map. |
| Spectating eliminated players | Not started |
| Rejoin after disconnect | Not started — a disconnect is a `Left` event. |
| Parties and queues | Not started — `/play` places one player at a time. |
| Results and records | Not started |
| Bot backfill | Not started — bots spawn only in shard 0's home hub and cannot fight. |
| Running-game rebalancing across cores | Not started — instances stay on the core they started on. |
| Cross-server transfer | Not started — designed: `StoreCookie` ticket plus `Transfer`. |
