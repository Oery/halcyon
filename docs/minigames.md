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

`Game` is the low-level interface; the hub implements it directly. Round-based
games implement `Match` instead.

## Matches

A `Match` declares its `MatchRules` and implements only the round.
`MatchRunner` (`game/matches.rs`) adapts it to `Game` and owns the rest:

```
Gathering ──(min_players present)──► Countdown ──(countdown_seconds)──► Running
    ▲                                    │                                 │
    └────(below min_players)─────────────┘                    tick() returns Some(Outcome)
                                                                           ▼
            Closing ◄──(send everyone to the hub, close)── Ending(end_seconds)
```

| Stage | Framework | Game hook | `phase()` |
| --- | --- | --- | --- |
| Gathering | Announces arrivals `name joined (n/max)`; waits for `min_players`. | `on_arrive(ctx, player, waiting)`: placement before the client loads (default: map spawn, protected). | `Open` |
| Countdown | Announces the start and the last three seconds; drops back to Gathering below `min_players`. | — | `Open` |
| Running | Forwards events. | `on_start(ctx, roster)` once, then `on_event` and `tick` each step until `tick` returns an `Outcome`. | `Running` |
| Ending | Announces the winners, waits `end_seconds`, sends everyone to the hub and closes. | — | `Ending` |

An instance whose players all leave closes at any stage. Events before the
round starts are not forwarded.

Gathering is deliberately separate from the round: its only output is the
starting roster passed to `on_start`. See [Queue lobbies](#queue-lobbies).

## Writing a game

1. Implement `Match` in `game/<name>.rs`:

   ```rust
   #[derive(Default)]
   pub struct KingOfTheHill { scores: HashMap<i32, u32> }

   impl KingOfTheHill {
       pub const KIND: &str = "koth";
   }

   impl Match for KingOfTheHill {
       fn kind(&self) -> &'static str { Self::KIND }
       fn rules(&self) -> MatchRules {
           MatchRules { min_players: 4, max_players: 12, countdown_seconds: 10, end_seconds: 5 }
       }
       fn on_start(&mut self, ctx: &mut GameCtx<'_>, roster: &[i32]) {
           // spawns, rules, loadouts
       }
       fn on_event(&mut self, ctx: &mut GameCtx<'_>, event: GameEvent) {}
       fn tick(&mut self, ctx: &mut GameCtx<'_>) -> Option<Outcome> {
           // objectives; Some(Outcome { winners }) ends the round
           None
       }
   }
   ```

2. Register it in `game/mod.rs`: add `KIND` to `KINDS` and an arm to
   `create` returning `MatchRunner::new(KingOfTheHill::default())` and its
   map (optionally through an environment override, like `HALCYON_FFA_MAP`).
3. `/play <kind>` now finds or starts it, with command completion.

`game/ffa.rs` is the reference implementation; `ring_spawn` places players on
a circle. A game that is not round-based (like the hub) implements `Game`
directly and manages `phase()`, `send_to` and `close()` itself.

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
| `ffa` | `Match` for 2–8 players: 5 s countdown, PvP until one player is left, back to the hub after 5 s. Map: `HALCYON_FFA_MAP`. |

Both default to the `Parkour Spiral` map.

## Queue lobbies

Planned direction, not implemented: players will gather in dedicated queue
lobbies, as on most minigame networks, rather than inside the match instance.
A queue lobby collects players for a game kind (and later parties, skill
brackets and bot backfill after a timeout), then starts a match instance with
the roster and moves everyone in together. Until then, gathering happens in
the match instance itself.

Design constraints this places on today's code:

- Keep gathering out of `Match`. A game sees only `on_arrive` placement and
  the `on_start` roster, so a runner can later start from a roster gathered
  elsewhere: wait for those players to arrive, then count down.
- Keep start decisions (minimum players, countdown) in `MatchRules`, readable
  by whatever does the gathering.
- Queues will span shards, since players wait in hubs on every core. Each game
  kind's queue needs one owner (for example a fixed shard), reached over the
  same channel mesh that hands connections between shards.

## Capability tracker

| Capability | Progress |
| --- | --- |
| Game definition and registration | Done — `Game` and `Match` traits, `KINDS` and `create` in `game/mod.rs`. |
| Match lifecycle | Done — `MatchRunner` gathers to `min_players`, counts down, runs the round until an `Outcome`, announces winners, returns players and closes. |
| Joining a running match | Not started — matches accept players only before the round starts. |
| Queue lobbies | Not started — planned; see [Queue lobbies](#queue-lobbies). |
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
| Round timers | Partial — countdown and end delay come from `MatchRules`; in-round timers count steps (`SIMULATION_HZ` per second). |
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
| Parties | Not started — `/play` places one player at a time. |
| Results and records | Not started |
| Bot backfill | Not started — bots spawn only in shard 0's home hub and cannot fight. |
| Running-game rebalancing across cores | Not started — instances stay on the core they started on. |
| Cross-server transfer | Not started — designed: `StoreCookie` ticket plus `Transfer`. |
