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
start ────────► on_create(ctx)         once, before anyone can join: build terrain
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
| `scoreboards()` | Reliable shared and personal sidebar, below-name and Tab score displays; see below. |
| `set_loadout(player, slots, selected)` | Queued inventory replacement. Queue it before a `reset` in the same step: a fresh life drops older queued actions. |
| `set_blocks(cells)` | Trusted terrain edits through the instance's overlay, applied immediately in bounded batches. Shares the overlay's limits (16,384 changed cells, 1,024 per column). |
| `send_to(player, destination)` | Moves the connection to `Hub`, `Matchmake(kind)` or `Instance(id)`, on any shard. |
| `close()` | Stops the instance once it is empty. |

Queued calls (`reset`, `set_loadout`, `teleport` after spawning) share the player's 32-entry
action queue and apply on the next step.

## Scoreboards

`ctx.scoreboards()` edits instance-owned scoreboard state immediately. The
connection publishes changes reliably within its existing writer budgets.
Games keep the returned `ObjectiveId` handles, publish updates from game events,
and explicitly remove objectives they no longer need. IDs belong to one
instance and become invalid after removal.

An objective has a title, integer or hearts rendering, an optional number
format and scores keyed by players or custom entries. Games supply all values;
there are no automatic health, kill-count or other vanilla criteria.

```rust
use crate::scoreboard::{
    DisplaySlot, ObjectiveSpec, Score, ScoreHolder, ScoreboardError,
};

fn show_points(ctx: &mut GameCtx<'_>) -> Result<(), ScoreboardError> {
    let roster: Vec<_> = ctx.players().collect();
    let mut boards = ctx.scoreboards();
    let points = boards.create_objective(ObjectiveSpec {
        title: "Points".into(),
        ..Default::default()
    })?;
    for player in roster {
        boards.set_score(points, ScoreHolder::Player(player), Score {
            value: 10,
            ..Default::default()
        })?;
    }
    boards.display(DisplaySlot::BelowName, Some(points))?;
    boards.display(DisplaySlot::PlayerList, Some(points))?;
    // Store `points` in the game to update it later.
    Ok(())
}
```

`BelowName` is vanilla's score line **below** a player's name. `PlayerList`
adds scores alongside existing Tab roster entries; it does not add fake
players. Hearts rendering follows the client's behavior, chiefly in Tab.

Declare a sidebar's immutable layout as static Rust data:

```rust
use crate::scoreboard::{LineId, SidebarLayout, SidebarLine};

const SCORE_LINE: LineId = LineId::new(0);
const TIME_LINE: LineId = LineId::new(2);
static SIDEBAR: SidebarLayout = SidebarLayout::new("My minigame", &[
    SidebarLine::new("Score: 0"),
    SidebarLine::new(""),
    SidebarLine::new("Time: 60s"),
]);
```

Create a shared or personal view of that layout during admission or round
initialization, then select its sidebar slot:

```rust
let mut boards = ctx.scoreboards();
let personal = boards.create_personal_sidebar(player, &SIDEBAR)?;
boards.display_for(player, DisplaySlot::Sidebar, DisplayOverride::Objective(personal))?;
// Alternatively: create_sidebar(&SIDEBAR), then display(Sidebar, Some(id)).
```

The minigame owns its values. An event updates both its game state and the
corresponding scoreboard line; no polling, text comparison or previous-value
tracking is needed:

```rust
// Inside the scoring event handler:
self.score += 1;
ctx.scoreboards().update_line(
    self.sidebar, SCORE_LINE, format_args!("Score: {}", self.score),
)?;

// Inside the game's seconds-timer event handler:
self.seconds_left -= 1;
ctx.scoreboards().update_line(
    self.sidebar, TIME_LINE, format_args!("Time: {}s", self.seconds_left),
)?;
```

`format_args!` is consumed synchronously. Both temporary text and encoding
scratch use bounded stack buffers; no rendered `String` is retained. Each
explicit call publishes a new line revision, even for identical text. Only
the latest immutable encoded packet is kept for delivery; slow clients
coalesce to it. Unaffected lines and the title are not rebuilt.

`update_lines(objectives, line, format_args!(...))` encodes a common event
update once for multiple views of the same layout. Pass a clonable iterator
of objective IDs, such as `self.sidebars.values().copied()`. FFA uses this to
publish the remaining-player count to all personal views.

The layout fixes the title, initial labels, ordering and styles at compile
time. Encoding happens once per instance/layout when its wire name is known.
Every view shares that immutable cached base. Per-view updates remain
separate; late viewers receive the base's unchanged entries and the latest
updates, without replaying event history. Updates never modify the base.

Layouts support up to 15 lines, including duplicates and blank lines, with
ordering numbers hidden. `SidebarLine.style` and `SidebarLayout.title_style`
accept constant `Style` values with a color and decorations; `Style::PLAIN`
is available for constant struct initialization. A `LineId` is its zero-based
position, checked by `LineId::new` during constant evaluation. Updating a
position absent from the chosen layout returns `ScoreboardError`.

Static layouts occupy only `Sidebar`. To change the layout between game
phases, create a view of another static layout and select it. The former
`sidebar` replacement helper and owned-text `set_sidebar_line` API have been
replaced by static creation and `update_line`.

Each viewer inherits the instance's three shared slots. `display_for` can
override each slot independently with `Objective(id)`, `Hidden`, or `Inherit`.
Shared objectives can also be selected for specific viewers, for example to
show a team's statistics only to its members. Personal objectives can be
selected only by their owner. Only selected objectives are sent to a viewer.

The general objective API still supports `Text` inputs with styled literal
`Span`s: a static vanilla color or `#rrggbb`, bold, italic, underline,
strikethrough and obfuscation. Styles are explicit per span.
`Score.display` supplies a custom label for a sidebar entry; use a stable
`ScoreHolder::Entry(key)` independent of that label. Use player holders for
below-name and Tab scores. `NumberFormat` supports blank numbers, styled
integers and fixed text, on an objective or individual score. An absent score
format inherits the objective's format.

`set_score` explicitly publishes a player or custom entry, without comparing
it to a retained value. General objectives can occupy any of the three slots.
`update_objective` changes their title/rendering/number format in place.
`reset_score` removes one score; `remove_objective` removes its scores and
slot assignments. Removing a personally selected objective hides that slot;
call `Inherit` explicitly to resume the shared display. `display(slot, None)`
clears a shared slot while keeping personal overrides.

Static layouts reject objective-property changes, generic score writes and
individual score removals. Their only mutable data is the separate encoded
line-update state. `remove_objective` also removes an entire static view.

Each instance allows 512 objectives and 16,384 total score entries. Custom
entry keys contain 1–64 bytes; each text component allows up to 64 spans and
4 KiB of encoded NBT. Invalid operations return `ScoreboardError` and retain
the previous state, including formatting overflow or failure. Multi-view
updates validate every target before publication. Slow writers coalesce to
current encoded state rather than accumulating score events.

Personal objectives, viewer overrides and scores keyed to a departing player
are cleaned up automatically. Ordinary respawn retains displays; an instance
transfer clears them before the destination initializes its own displays.
Teams, name prefixes/suffixes, Tab display names/header/footer, scoreboard
commands and persistence are not provided by this API.

## Built-in games

| Kind | Behavior |
| --- | --- |
| `hub` | Protected lobby (PvP with `HALCYON_PVP=1`). Holds `HALCYON_HUB_CAPACITY` players (default 100, bots included); players fill the fullest hub with room across shards. Each shard keeps one permanent hub; overflow hubs stop after 60 s empty. Map: `HALCYON_HUB_MAP`. |
| `ffa` | `Match` for 2–8 players: 5 s countdown, PvP until one player is left, back to the hub after 5 s. Personal sidebar shows waiting status, then remaining players and alive/eliminated status. Map: `HALCYON_FFA_MAP`. |
| `boxes` | `Match` for 2–8 players: four 17×17 glass boxes built in the sky over the map, plus a gallery. After a 5 s countdown, fighters are paired into boxes in entity order (an odd one joins the last pair) with an iron sword and iron armor; a box is decided when one fighter is left, and the round ends when every box is. Eliminated players watch from the gallery. Map: `HALCYON_BOXES_MAP`. |
| `boxes_long` | `boxes` with a stone sword against full diamond armor: about sixteen hits to win instead of seven, so duels last two to three times longer. |
| `ladder` | One player against a sparring bot in a `boxes` box with the `boxes_long` kit. The bot starts at level 50 (`HALCYON_LADDER_LEVEL`); beating it makes the next bot 10 levels stronger, losing makes it 10 weaker, and the next fight starts the moment one ends. The game starts its own bot (`Game::sparring`), which leaves with the player. |

All default to the `Parkour Spiral` map. Every finished round prints one
line to stdout (`instance <id> (<kind>): <result> (<seconds>s)`).

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
| Loadouts and item grants | Partial — `set_loadout` replaces a player's inventory; no single-item grants. |
| Game mode changes | Not started in `GameCtx` — e.g. spectator for eliminated players. |
| Terrain edits from games | Partial — `set_blocks` and the `on_create` hook build terrain; signs, jukeboxes and openings exist on `Instance` only. |
| Titles, action bar and sounds | Not started |
| Scoreboards and sidebars | Done — shared and personal sidebar, below-name and Tab scores; static sidebar layouts with shared immutable bases, explicit event updates, stack formatting and custom number formats. Game-supplied values; no vanilla criteria or scoreboard commands. |
| Boss bars | Not started |
| Teams | Not started — membership, nametag colors, friendly fire. |
| Objectives and regions | Not started — capture zones, checkpoints, region enter/leave events. |
| Map metadata | Not started — named spawn points and regions per map. |
| Spectating eliminated players | Not started |
| Rejoin after disconnect | Not started — a disconnect is a `Left` event. |
| Parties | Not started — `/play` places one player at a time. |
| Results and records | Not started |
| Bot backfill | Partial — with `HALCYON_BOT_GAME=<kind>` bots queue from shard 0's hub like players and play `boxes` with a melee brain; no filling of human games after a timeout. |
| Running-game rebalancing across cores | Not started — instances stay on the core they started on. |
| Cross-server transfer | Not started — designed: `StoreCookie` ticket plus `Transfer`. |
