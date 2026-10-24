# Halcyon bots: expectations and architecture

Status: embodiment, motor, profiles and navigation are implemented (§8), and a melee brain with a combat bench plays box duels (§9); a general perception model, traits outside combat and team play are not. See `feature-progress.md` §20.

## 1. Goals and non-goals

**Goal.** A single player can queue an 8v8 minigame and get a game that feels like a real match:

- coherent;
- varied;
- winnable and losable;
- alive between action beats.

**Disclosure.** Bots are disclosed: players know that games are bot-filled. Success is not about deceiving anyone. It means that *in play, bots read as people*: no mechanical tells, no robotic movement, no identical behavior.

**Same rules as humans.** Bots act only through the shared action path (`PlayerAction`, `MovementInput`), with:

- the same physics and collision;
- no teleports or special movement;
- no information a vanilla client would not have.

A bot that wins must win the way a human could.

**Non-goals for now:**

- learned/ML policies (Conditional in §20);
- chat that imitates humans;
- vanilla mob AI.

First target game: **parkour race** (Parkour Spiral map). Movement is the foundation every other game builds on.

## 2. What "humanlike" means

These expectations are targets, not calibrated measurements: there is no human recording data. We get a bot working first, then add these behaviors over time, tuning by playing against bots and watching them.

### Motor

- **Camera turns** follow human profiles:
  - bell-shaped angular velocity;
  - duration that grows with angle and precision (Fitts-like);
  - overshoot then correction on large turns;
  - micro-jitter while "holding" aim;
  - discrete sensitivity steps.
  - Never instant snaps, never constant angular rates, never pixel-perfect stillness.
- **Keys** have reaction latency (about 150–300 ms depending on level and state) and human press durations.
  - Jumps take off slightly early or late relative to the ideal edge.
  - Strafe taps and sprint toggles have realistic timing.
- **Head and body:** idle looking around, glancing at nearby players, looking ahead along the path rather than at the feet.

### Perception

- Bots react only to what is in their field of view, unoccluded and in range.
- Attention is limited: a bot tracks a few things at once.
- Memory decays: a player seen 10 s ago is a stale estimate, not a live position.

### Decision

- Hesitation before hard jumps, misjudgements, and recoveries.
- Route choices vary: sometimes sub-optimal, never optimal-and-identical across bots.
- Two bots of the same level must not behave identically.

### Failure

- Bots fail at plausible points:
  - hard jumps;
  - after rushing;
  - when tilted or under pressure.
- Failures have aftermath: a pause, looking around, a retry or a safer line.
- Never impossible failures:
  - walking into a wall forever;
  - jittering in place;
  - running off an edge with no attempt to jump;
  - freezing.

### Idle and social

Bots:

- look around at spawn;
- wait at the start line;
- glance at or follow other players;
- jump or sneak idly;
- spectate after finishing.

### Acceptance criteria

1. Watching or playing alongside bots, we see none of the tells listed above (snaps, constant rates, identical bots, impossible failures).
2. Parameters such as reaction time and turn speed stay within hand-picked plausible ranges, and the ranges widen or shift with level.
3. A degenerate behavior (stuck, oscillating, wall-walking) is a bug with a regression scenario.

## 3. Layered architecture

Layers are listed bottom-up. Each one exposes a narrow interface to the layer above and runs at its own cadence. Simulation, network publication and bot decisions are already allowed independent cadences (AGENTS.md).

| Layer | Responsibility | Cadence |
| --- | --- | --- |
| L0 Embodiment | Being a player in the world | Every tick |
| L1 Motor | Humanlike input generation | Every tick |
| L2 Perception & memory | What the bot knows | ~5–10 Hz |
| L3 Skills | Reusable maneuvers | On demand |
| L4 Navigation & tactics | Where to go, how | ~2–5 Hz |
| L5 Game goals | What to achieve in this game | ~1 Hz |
| L6 Match director | Coherence of the whole match | Per match/event |

### L0 Embodiment

A bot is a registry player without a socket:

- **Registration:** `Lobby::register` (`crates/halcyon-server/src/lobby/mod.rs`).
- **Movement:** `Session::drive(Some(MovementInput))` (`lobby/session.rs`). This makes the owner step physics every tick via `MovementState::step` (`movement/physics.rs`).
- **Replication:** observers need nothing new; `visibility::Observer` replicates any active registry player.

Known engine gaps:

- **Activation:** solved without engine changes. Bots request their spawn teleport and confirm it like a client, which activates them.
- **Player Info:** no latency or game-mode fields (tab-list ping).
- **Skins:** bots have no texture properties.

### L1 Motor ("hands")

Turns intents into a per-tick `MovementInput` stream:

- "look at P"
- "hold forward, sprint"
- "jump at the edge"

It models a mouse and keyboard:

- minimum-jerk aim trajectories with Fitts-like durations;
- overshoot and correction;
- tremor noise;
- sensitivity quantization;
- key press/release latency and minimum hold times.

This layer carries most of the humanlike feel. Every higher layer goes through it, and none may set rotation or input directly.

### L2 Perception and memory

- Field-of-view and occlusion checks against the world (`BlockView`).
- Perceived facts are stored with a perception delay and timestamp.
- Memory decays, and the number of tracked entities is bounded by an attention budget.

### L3 Skills

Maneuvers with a success model that depends on level. Each one is executed via L1, never by shortcuts.

- **Parkour first:**
  - sprint-jump;
  - n-block gap jumps;
  - corner/neo jumps;
  - ladders;
  - landing control;
  - fall recovery.
- **Later:** aiming, strafing, bridging, item use.

### L4 Navigation and tactics

- A parkour-aware route graph with walk and jump edges. Jump reachability is derived offline from the real physics (`MovementState::step`), so it never disagrees with what a player can do.
- Local steering, and re-planning after failures.
- Route knowledge depends on level: weaker bots take safer, slower lines and know fewer shortcuts.

### L5 Game goals

Game-specific objectives. Parkour:

- finish;
- recover after a fall;
- decide whether to retry a hard jump or take the safe path.

Team objectives come later.

### L6 Match director

Keeps the match coherent and alive:

- fills slots and balances teams by level;
- spreads levels and traits so a lobby is varied;
- paces events to avoid dead stretches;
- handles bot lifecycle: fill at queue time, plausible join timing.

It chooses levels and traits when it creates bots, and never changes them afterwards. It never cheats: no rubber-banding through physics or hidden information.

## 4. Level, traits and state

These are three orthogonal axes.

- **Level** (scalar, e.g. 0–100) sets *capability* distributions:
  - reaction mean and variance;
  - aim precision;
  - jump-timing error;
  - route knowledge;
  - recovery speed.

  A level maps to a parameter distribution, and each bot samples its own values from it, so same-level bots differ.
- **Traits** are persistent per bot identity and independent of level. They change *choices*, not capability. Examples:
  - risk-taking;
  - patience;
  - curiosity/exploration;
  - sociability;
  - consistency;
  - tilt-proneness;
  - competitiveness.

  A bot identity (name, skin, seed) keeps its traits across games.
- **State** changes within a match: fatigue, tilt after repeated failures, confidence, focus. Behavior is never stationary across a game.

Level and traits are fixed when a bot is created and never change afterwards. Only state evolves during a match.

Level and trait profiles are data, not code: declarative, tunable and comparable in A/B runs.

## 5. Engineering constraints

- **Performance:**
  - Target: 100 participants per world, with an 8.33 ms tick in the future.
  - L0–L1 are O(1) and allocation-free per tick.
  - Higher layers are amortized, cadence-limited and budget-capped per tick.
  - Measure tail latency rather than averages.
- **Determinism:**
  - Bots are seeded.
  - Scenario harnesses (map, spawn, profile, seed) reproduce runs exactly, for tests and tuning.
  - Tests follow the `src/tests/tests_<feature>.rs` convention.
- **Tuning:**
  - No human recordings are planned. Parameters are hand-tuned by observation.
  - Behaviors are added incrementally on top of a working bot.

## 6. Milestones

1. **Embodiment:** done. Bots are visible, register and respawn like clients. Still missing: animation packet.
2. **Motor layer:** first version done, hand-tuned.
3. **Parkour skills and route graph:** navigation done (§8). On Parkour Spiral, bots climb through checkpoint 22 of 35 (§8, "Parkour Spiral status").
4. **Level, trait and state profiles:** several distinct bots racing together.
5. **Match director and queue/fill policy** (depends on §18 matches).
6. **Combat and team-game layers:** melee brain, combat bench and tuner done (§9); team play not started.

## 7. Decisions and open questions

Decided:

- Level and traits are fixed when a bot is created. There is no adaptive difficulty.
- Humans replacing bots in a running match is out of scope and not planned.
- No human input recordings: behaviors are added incrementally and hand-tuned.

Open:

- How is disclosure shown: tab-list prefix, queue UI, name suffix? To be determined.

## 8. Navigation (implemented)

Code: `crates/halcyon-server/src/bot/`. Tests: `src/tests/tests_bot_*.rs`. Real-map scenario and survey tools: `src/bench/bot_nav.rs`.

### Graph

- **Nodes** (`nav/stand.rs`): standable cells. A node is the block cell containing the feet; support comes from that cell (slabs, carpets, lily pads, stair steps) or the block below. Lava, unsupported and contextual blocks are never nodes. Ladders and vines are climb nodes.
- **Candidates** (`nav/moves.rs`): from geometry alone, a node proposes:
  - walks and step-ups to its 8 neighbours;
  - drops of up to 3.6 blocks that cost no health (health does not regenerate), or up to 14 onto hay, honey or beds for at most one heart;
  - ladder and vine climbs;
  - jumps of up to 5 blocks horizontally and +1.3 up, onto floors or onto a ladder, which is grabbed;
  - bounces off one slime pad, or two in a row, onto floors the rebound reaches. Bots hold jump coming down onto a pad and face where the rebound goes next, so the sprint jump's boost carries them on.

  Cheap filters remove jumps through solid blocks and jumps over continuous floor.
- **Validation:** a candidate becomes an edge only after a simulation succeeds. The simulation runs the real player physics (`MovementState::step`) under the **same closed-loop controller** live bots use (`nav/follow.rs`), from rest, with steady hands: exact aim and key timing, but the camera turns at a level-100 bot's rate and easing, as the live motor does. It must then settle on the target.
  - Jumps try several plans: run-up, takeoff point, sprint.
  - Each accepted jump gets a **robustness** score: the success rate under perturbed aim (±3°), early takeoff and a one-tick-late jump.
- **Laziness:** the search queues candidates at optimistic costs and validates one only when it reaches the front of the queue. Most candidates are never simulated.
- **Sharing and invalidation:** one `NavService` per lobby caches stands, candidates and validations for every bot. Arena column revisions drop entries around edited columns.

### Search and execution

- **Search** (`nav/search.rs`): weighted A*. The heuristic counts horizontal distance plus about 15 ticks per block of height. Edge cost is ticks, plus risk aversion × (1 − robustness), plus remembered failures. Searches resume across ticks under a shared validation budget (`HALCYON_BOT_NAV_MS`, default 8 ms per tick). They return a partial route to the closest cell when the goal is unreachable.
- **No waiting on the planner:** a bot never stands still while a search runs.
  - It searches the next stretch (past a checkpoint, or on from a partial route's end) while walking the current one.
  - With no route yet, it explores: one plausible move at a time toward its target, using validated edges and skipping long drops, fragile jumps and recently visited cells. Exploring validates only walks and drops (a few even when the shared budget is spent); jumps are left to the search, as they cost many simulations.
  - When the search delivers, the bot joins the route where it stands, or finds a short way back onto it. Otherwise the search restarts from where the bot is, using cached validations.
  - When no route exists, it explores a few moves before searching again.
- **Execution:** the brain (`brain.rs`) follows a route edge by edge with the same controller:
  - jumps back up to the run-up point, line up, hesitate, then commit;
  - walks steer cell to cell, leaning along a wall to round its corner on diagonals;
  - movement keys are chosen relative to where the camera points, so a bot still turning strafes straight to its mark instead of walking an arc around it;
  - climbs push into ladders or hold jump on vines;
  - a grabbed ladder is held (sneaking, steering out the drift) before climbing on.

  The motor (`motor.rs`) adds humanlike error: eased, rate-limited turning, tremor, committed aim error and occasional late jump presses.
- **Failure handling:** a failed edge is remembered and penalised for that bot, which replans as soon as it lands, without a pause. A search started nearby survives the fall. In water, the bot swims to the nearest floor (still searching) and replans.
- **Cadence:** live bots decide once per simulation step, woken by the owner's publication.

### Profiles

`HALCYON_BOT_LEVEL` is `n` or `min-max` (default `20-90`). Each bot draws its level once. The level scales turn rate, tremor, aim error and tolerance, late-press probability, hesitation and risk aversion.

### Tools

| Command / variable | What it does |
| --- | --- |
| `cargo test --release -p halcyon-server spire_climb_scenario -- --ignored --nocapture` | Headless bots on a real map. Reports route existence, simulation cost, per-bot progress and, when the route is partial, a frontier report. |
| `survey_map`, `survey_layers`, `survey_box`, `survey_inventory`, `survey_raised_water`, `survey_lily_pads`, `geometric_route`, `trace_edge` | Map forensics: height maps, slices, block inventories, optimistic routes and tick-by-tick traces of one edge. |
| `HALCYON_DIAGNOSTICS=1` | Prints a `bots:` line every 5 s: modes, ticks stalled and moves explored, searches finished and their mean duration, failures by cause, heights and navigation cache and cost. |
| `HALCYON_BOT_TRACE=<name>` or `all` | Traces one bot (e.g. `Bot45`) or every bot: mode, position, route and current edge each second, and the cached moves around it when a search fails. |
| `HALCYON_BOT_CHECKPOINT=n` | Starts every bot on the n-th checkpoint (from 1), heading for the next, to work on later parts of a course without waiting for the climb. A fallen bot returns there. |
| `geometric_route` with `HALCYON_ROUTE_FROM`/`HALCYON_ROUTE_TO` | One leg, e.g. between checkpoints: optimistic route, near misses and a reached/unreached height map. |
| `edge_success_rate`, `trace_bot`, `survey_checkpoints`, `survey_approaches` | Live-controller success rate of one edge per level, a traced headless bot, the checkpoint list, floors around a goal. |
| `route_fidelity` (`HALCYON_FIDELITY_TRIALS`) | Every edge of the route from spawn, run under the live motor at levels 0, 50 and 100: how often the planner's promises hold, and the edges a level-100 bot fails. |

### Checkpoints, bounces and resets

- **Checkpoints:** gold (light weighted) pressure plates within 128 blocks of spawn are the course's checkpoints, ordered by height (`NavService::detect_checkpoints`). Each bot routes to its next plate and advances when it stands on one. It never skips a plate: if no route exists, it waits and retries. Set `HALCYON_BOT_NO_CHECKPOINTS` to aim straight for the top instead.
- **Bounces:** a slime block within jumping reach becomes a bounce edge to floors its rebound reaches, possibly through a second pad. The bot lands on each pad's far side, holding jump and facing where the rebound goes next; validation simulates the result like any other edge.
- **Resets:** a bot trapped in water for 30 s, or stranded (no route, the same dead end, or nowhere it can locate itself, three times in a row), asks to be returned to its last checkpoint, or to spawn. The server teleports it. This stands in for the course's own fall handling, which Halcyon does not run for humans yet.

### Parkour Spiral status

The planner connects every leg from spawn to checkpoint 22 of 35. Live and headless bots of levels 40–90 climb that far, weaker ones more slowly and with more falls. Along the way they handle:

- lily pads, slabs, stairs, carpets, sand steps and floating sand that settled at startup;
- 4-block gaps through a momentum hop on a short runway;
- diagonals around a wall's corner;
- wooden doors (opened), closed iron doors (avoided);
- ladders and vines, including jumps onto ladders across lava;
- a 9-block drop onto hay;
- single and double slime bounces.

Leg 22 → 23 and several later legs remain partial. Extending the climb further is deliberately on hold: the next work is making bots better rather than going higher.

### Known gaps

- Swimming routes; bubble columns and scaffolding (supported by physics, not planned).
- Momentum carried across edges, except the single hop; neo jumps; jumps off ladders.
- Redstone (pressure-plate or button doors).
- A game rule returning fallen humans to checkpoints (bots have a stand-in).
- Validation runs without the one-tick observation latency live bots have, so a few edges validate better than they execute.
- Perception: bots know the terrain but not other players.

## 9. Combat

Code: `bot/fighter.rs` (brain), `bot/motor.rs` (hands), `bot/profile.rs` (level knobs). Bench: `src/bench/bot_combat.rs`. Tests: `src/tests/tests_bot_fighter.rs`, and a duel smoke test in `tests_instance.rs`. The techniques follow [minecraft-combat.md](minecraft-combat.md).

### Playing games

- **Games:** bots follow a game's `Destination` like a client does. They leave the instance and register in the new one on shard 0 under the same entity ID. In the home hub they climb the course; elsewhere the fighter brain runs.
- **Queueing:** with `HALCYON_BOT_GAME=<kind>` (e.g. `boxes`), each bot waits 3–15 s in a hub, matchmakes into that game and returns to the hub after the round, over and over, so games run with no human online.
- **Engagement:** a fighter starts fighting when the round announces `Fight!` in chat, and stops when it dies.

### Perception

A fighter (`Senses`) knows only what a client shows:

- other visible, living players, with their latest **hurt flash** and **arm swing** (`Registry::cues`);
- its own health;
- whether it was just hurt.

Opponents' health is never visible to it.

- **Targeting:** the target is the current one while reachable, else the nearest within 24 blocks with no solid block on the line between them. Glass walls therefore separate boxes.
- **Reaction:** positions are seen through a reaction delay of 6 − 3 × skill ticks, drawn per decision between the neighbouring whole ticks. A level-100 bot reacts in about 150 ms, a level-0 one in 300 ms.

### Aim and clicks

- **Aim:** the bot predicts the target from what it saw a reaction ago to the frame the client shows now, one tick old. It extrapolates the target's 3D motion by `lead` × (lag − 1).
- **Smooth pursuit:** the motor adds a feed-forward of the aim point's sweep to its eased, rate-limited turn. Vertical turns are limited to 60% of horizontal ones. Without this, a camera chasing a strafing target trails it by 30–40° and never clicks.
- **Clicks:** a click answers what the crosshair showed `click_lag` ticks earlier (0.7 at level 100, 3.2 at level 0, drawn per decision). It hits whatever is under the crosshair now: the target as rendered, within 3 blocks with nothing solid in between. If the crosshair has slid off, the click is a whiff: an empty swing that still resets the charge.

### Techniques

`Tactics` is fixed at creation from the level and two per-bot traits, aggression and patience. `MASTER` holds a strong player's values, found with the tuner; lower levels scale down from it.

| Technique | What the fighter does |
| --- | --- |
| Charge timing (half-swing) | Plans the click for the first tick the server counts as strong (charge > 0.9: tick 12 of a sword's 12.5), with a per-swing spread of 0.5 ticks at level 100 and 2.5 at level 0. The server counts the charge from the tick after the click. |
| Damage windows (i-frames) | Holds a swing while the target's hurt flash is under 10 ticks old, unless the swing would be a falling critical. |
| Sprint resets | After a sprint-knockback hit, the adaptive reset s-taps (backs off 1–3 ticks) when the target stays inside 2.2 blocks, keeps sprinting when chasing, and otherwise w-taps. |
| Criticals | Jumps about 8 ticks before the planned click, when the run-in will close to reach by then. Releases sprint on the way down and holds the swing until falling, unless a charged opponent is about to hit first. A jump reset whose descent covers the planned click doubles as a critical jump. |
| Jump reset | Jumps on the tick it is hit while grounded, so the sprint-jump boost toward the attacker cancels part of the knockback: about 1.5 blocks taken instead of 1.8 at level 75. |
| Combos | Chases a target its hit knocked away (receding faster than 0.08 blocks per tick). |
| Evasion | Dodges sideways for 3–6 ticks when a charged opponent swings while its own charge is not back. |
| Spacing and retreats | Patient bots hover just outside reach while recharging, wait at the edge when they will be charged first, and back off after taking two hits in a row or below 6 health. |
| Strafing | Changes direction every 8–30 ticks, away from walls, sometimes holding still. With a wall close behind, it never backs up further and slips out diagonally toward the side with room. |
| Hit-selecting | Waits for a charged opponent to swing first. Bots doing it lose 73% of duels against the same tactics without it, so `MASTER` leaves it off. |

### Bench

`cargo test -p halcyon-server --release combat_duel_benchmark -- --ignored --nocapture` runs duels under the real owner rules:

- two fighters in a `boxes` box, with the game's kit;
- publication, then decisions that land on the next step, as for live bots;
- simulated time, and a draw after 90 s.

Every matchup runs half its duels with the entity IDs swapped. `HALCYON_COMBAT_SCENARIO` picks one scenario, `HALCYON_COMBAT_DUELS` sets duels per matchup (default 200), `HALCYON_COMBAT_KIT=long` fights in the `boxes_long` kit, `HALCYON_COMBAT_COUNTERS=1` prints how often each technique was tried, and `HALCYON_BENCH_REPORT` writes JSON. 2,000 duels take about 4 s.

| Scenario | Question |
| --- | --- |
| `baseline` | The fighter against the first version (`Tactics::basic`), both level 75. |
| `mirror`, `bias` | Same level or identical bots: expect about 50% after side swaps. |
| `ladder` | Levels 0, 25, 50, 75 and 100, round robin. |
| `ablation` | Level-75 tactics against the same with one technique changed. |
| `probe` | Level-100 tactics with one knob at its level-50 value. |
| `isolate` | Strong hands with middling tactics, and the reverse, against a plain level 50. |

Each side reports:

- clicks per second, accuracy, mean strength, sprint-knockback and critical shares;
- hit distance, combo lengths, knockback taken and jump-reset share;
- mean gap, wall time, aim error at click, turn speeds and click-interval spread.

It prints any **tell** as `TELL:`:

| Tell | Threshold |
| --- | --- |
| Camera snap | a turn faster than 2400°/s |
| Frozen aim | over 20% of engaged ticks without any rotation |
| Metronome clicks | a click-interval spread under 0.5 ticks |
| Jitter | over 3 strafe reversals per second |
| Wall hugging | over 35% of engaged ticks against a wall |
| Standing still | over 25% of engaged ticks with no movement key |
| Pixel aim | a mean aim error at click under 0.5° |
| Never misses | over 98% of swings landing |

Tools:

- `trace_duel` (`HALCYON_DUEL=<levelA>,<levelB>,<seed>`, optionally `HALCYON_DUEL_TACTICS=<level>` or `HALCYON_DUEL_CRITS=<p>`) prints one duel tick by tick.
- `tune_tactics` runs a coordinate search over `MASTER`'s knobs. A step is kept when it beats the current and recent best tactics significantly, without tells (`HALCYON_TUNE_DUELS`, `HALCYON_TUNE_ROUNDS`).

### Results

300 duels per matchup:

| Matchup | Higher level wins |
| --- | --- |
| 25 vs 0 | 83% |
| 50 vs 25 | 82% |
| 75 vs 50 | 86% |
| 100 vs 75 | 63–72% |
| 100 vs 50 | 90% |
| Level 75 vs the first version at level 75 | 89% |
| Mirrors and swapped identical bots | 47–52% |

No tells fire. At level 100, 94–97% of swings land against strong opponents, crits are 7–20% of hits depending on the opponent, and duels between strong bots last 5–6 s in the `boxes` kit and 13–15 s in the `boxes_long` kit.

### What matters, measured

- **Hands decide most fights.** Prediction and pursuit are worth 80% and 68% against bots without them. Reaction time and click timing make up the rest of what separates levels (`isolate`: level-100 hands alone win 88% against level 50, and level-100 tactics alone 89%).
- **Equal bots trade.** Both swing on charge and the server resolves attacks against positions up to 6 blocks away, so a hit is almost always answered. Hits are lost to re-approach time, and about a third of the possible swings never happen.
- **Individual techniques move results by only a few percent between equal bots.** Jump resets, crits, sprint resets and evasion each fall within a few percent in ablations. They remain because they read as human and punish weaker players; criticals, for example, are 20% of a level-100 bot's hits against beginners.
- **Crits and jump resets compete for airtime.** With a sword, a jump reset right after a trade lands before the charge is strong, so it cannot become a critical. Criticals need the jump about 8 ticks before the click.

### Known gaps

- No field-of-view or memory model.
- No blocking, rods, bows or item use.
- No team awareness.
- Fighters do not route around obstacles; box duels do not need it.
- The bench measures bots against bots; strength against humans still needs live play.
