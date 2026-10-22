# Halcyon bots: expectations and architecture

Status: design spec. No bot code exists yet; see `feature-progress.md` §20.

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

- **Activation:** players become visible only after a confirmed teleport. `Session::activate` is `#[cfg(test)]` only, so bots need a real activation path.
- **No arm-swing / entity-animation packet,** and no observer-side hurt animation.
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

1. **Embodiment:**
   - a bot that is visible in the tab list and world, stands, looks around and walks;
   - activation path and animation packet.
2. **Motor layer:** humanlike camera and key timing, hand-tuned.
3. **Parkour skills and route graph** on Parkour Spiral: bots complete the course at different levels.
4. **Level, trait and state profiles:** several distinct bots racing together.
5. **Match director and queue/fill policy** (depends on §18 matches).
6. **Combat and team-game layers.**

## 7. Decisions and open questions

Decided:

- Level and traits are fixed when a bot is created. There is no adaptive difficulty.
- Humans replacing bots in a running match is out of scope and not planned.
- No human input recordings: behaviors are added incrementally and hand-tuned.

Open:

- How is disclosure shown: tab-list prefix, queue UI, name suffix? To be determined.
