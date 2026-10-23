# Halcyon feature progress

Last reviewed: 2026-10-22. Target: Java Edition 26.3.

All 335 features from the [feature assessment](halcyon-server-features.md), in the same order. This tracks available behavior; the assessment records scope and rationale.

**Done** = implemented within the stated scope. **Partial** = only the listed portion is implemented. **Not started** = intended capability with no implementation. **Conditional** = unimplemented; needs a game or operational requirement. **Excluded** = out of scope.

Update this tracker when feature support changes. Packet definitions, static client-visible values and visual presence alone do not count as gameplay support.

## 1. Connections and compatibility

| Feature | Progress |
| --- | --- |
| Server-list status | Done |
| Status latency measurement | Done |
| Protocol negotiation | Partial — handshake intents; no version rejection. |
| Online authentication | Done |
| Offline-mode identity | Excluded |
| Profile properties | Done — names and skins. |
| Encrypted transport | Done |
| Packet compression | Not started |
| Configuration phase | Done |
| Reconfiguration | Conditional |
| Client preferences | Partial — view distance and skin settings. |
| Resource-pack delivery | Not started |
| Keepalives and inactivity | Partial — liveness timeouts; no AFK policy. |
| Disconnect handling | Partial — session cleanup; no match handling. |
| Incoming server transfers | Conditional |

## 2. Players, movement, and lifecycle

| Feature | Progress |
| --- | --- |
| Survival mode | Partial — player lifecycle and damage rules; no survival loop. |
| Creative mode | Partial — mode switching, flight, invulnerability and instant block breaking with no drops; creative item acquisition remains unavailable. |
| Adventure mode | Partial — mode switching and client synchronization; lobby gameplay rules remain shared. |
| Spectator mode | Done — always-on flight through terrain and fluids, invulnerability except void rules, no block, item or inventory interaction, visibility only to other spectators, viewing through a player's camera (attack to follow, sneak to leave) and spectator-menu teleports. Portals are not available. |
| Hardcore rules | Excluded |
| Ground movement | Done — 20 TPS input-driven ground physics and collision-checked human predictions; no speed/flight enforcement. |
| Collision and stepping | Partial — static shapes and stepping, offset bamboo/dripstone/sulfur spikes, scaffolding and powder-snow player contexts, retained dripleaf tilt shapes, closed shulker boxes and empty moving-piston placeholders; animated block entities and scheduled dripleaf tilting unavailable. |
| Sprinting and sneaking | Done — ground speed, crouch clearance and sneak edge protection. |
| Jumping and falling | Done — ground jumps, gravity, air control, jump timing and supported landing effects. |
| Swimming and crawling | Done — water travel, sprint swimming, currents, shoreline exits and swim/crawl pose transitions; air and drowning remain unavailable. |
| Climbing | Done — ladders, matching open trapdoors above ladders, vines and scaffolding, including sneaking to descend scaffolding. |
| Movement surfaces and media | Partial — dry surfaces, honey sliding, landing modifiers, berry-bush/cobweb slowdown, powder-snow sinking and leather-boot walking, water/lava travel, bubble-column lift/drag and surface launches, ladder/vine/scaffolding climbing, aquatic plants and supported waterlogged geometry; lava burning and freezing unavailable. |
| Flight permissions | Partial — Creative permits client-toggled flight; leaving Creative revokes it. |
| Movement abilities and attributes | Partial — Creative flight and instant-break abilities synchronize to the client. |
| Teleportation | Done — validated destinations and position corrections. |
| Initial spawning | Partial — imported spawn with bounded recovery to nearby clear supported terrain; no team assignment. |
| Personal respawn points | Partial — per-player destinations; no team assignment. |
| Death processing | Partial — death state and messages; no drops or match callbacks. |
| Respawning | Done — same-world respawn/reset; inventory retained. |
| Dimension transitions | Not started |
| Player poses and actions | Partial — standing, crouching, swimming/crawling, dying, visible arm swings and item-use hand state. |
| End completion flow | Excluded |

## 3. Combat and survival

| Feature | Progress |
| --- | --- |
| Melee targeting | Not started |
| Attack cooldowns | Not started |
| Critical hits | Not started |
| Sweeping attacks | Not started |
| Mace smashes | Conditional |
| Spear attacks | Conditional |
| Knockback | Not started |
| Shields and blocking | Not started |
| Ranged weapon use | Not started |
| Projectile motion and hits | Not started |
| Projectile ownership | Not started |
| Damage types and attribution | Partial — fall, void and game damage with optional source. |
| Damage cooldowns | Done — shared damage immunity window. |
| Armor and protection | Not started |
| Health and absorption | Partial — health; no absorption. |
| Healing and regeneration | Partial — explicit healing; no regeneration. |
| Hunger, saturation, and exhaustion | Not started |
| Eating and drinking | Not started |
| Status effects | Not started |
| Splash and lingering effects | Conditional |
| Environmental damage | Partial — fall damage with surface modifiers, pointed-dripstone tip impacts, powder-snow cushioning, void damage and block suffocation; inside-block fall resets supported, but thorn damage and freezing unavailable. |
| Air and exposure state | Not started |
| Death prevention | Not started |
| Enchantment effects | Conditional |
| Player-versus-player rules | Not started |

## 4. Items and inventory

| Feature | Progress |
| --- | --- |
| Item stacks | Done |
| Item components | Partial — defaults plus damage, unbreakable, name, lore, dye and glint overrides. |
| Player inventory | Done — storage, hotbar, armor, offhand and cursor. |
| Equipment changes | Partial — held items, armor and offhand synchronized; no attribute effects. |
| Inventory clicks | Done — ordinary rearrangement; crafting and dropping disabled. |
| Inventory reconciliation | Done |
| Container sessions | Not started |
| Container properties | Not started |
| Item pickup | Not started |
| Item dropping | Not started |
| Dropped-item lifecycle | Not started |
| Durability and breakage | Not started |
| Item-use cooldowns | Not started |
| Nested item storage | Conditional |
| Ender chest storage | Not started |
| Books and written content | Conditional |
| Fishing | Conditional |
| Navigation item targets | Not started |
| Creative item acquisition | Conditional |

## 5. Crafting, processing, and trading

| Feature | Progress |
| --- | --- |
| Grid crafting | Conditional |
| Crafting outputs and remainders | Conditional |
| Recipe book interaction | Conditional |
| Furnace processing | Conditional |
| Specialized cooking | Conditional |
| Brewing | Conditional |
| Enchanting table | Conditional |
| Anvil | Conditional |
| Grindstone | Conditional |
| Smithing | Conditional |
| Stonecutting | Conditional |
| Loom | Conditional |
| Cartography table | Conditional |
| Merchant transactions | Not started |
| Trade economics | Excluded |
| Bartering | Excluded |

## 6. Blocks and interaction

| Feature | Progress |
| --- | --- |
| Block states | Partial — imported states, authoritative queries and bounded game-owned terrain edits; no saving. |
| Block placement | Not started |
| Block breaking | Partial — instant Creative breaking with reach and terrain-capacity checks, paired-block removal, water preservation and no drops; Survival digging remains disabled. |
| Break feedback | Not started |
| Block drops | Conditional |
| Interaction validation | Partial — door, trapdoor and fence-gate use checks reach, hand and sneaking with held items; Creative breaking checks mode, reach and player life; unsupported predictions are corrected. |
| Predicted-action correction | Partial — block states and sequence acknowledgements restore rejected edits; collision corrections resend nearby terrain. |
| Neighbor and shape updates | Partial — sand/gravel support changes; no general shape updates. |
| Block entities | Not started |
| Doors, trapdoors, and gates | Partial — hand-operated doors, trapdoors and fence gates; water in waterlogged trapdoors does not flow when opened; no sounds for other players. |
| Beds and respawn anchors | Conditional |
| Straw beds | Conditional |
| Signs | Not started |
| Shelves and storage interactions | Conditional |
| Jukeboxes | Not started |
| Buckets and fluid containers | Conditional |
| Tool transformations | Conditional |
| Farming and harvesting | Conditional |
| Archaeology | Excluded |
| Beacons and conduits | Conditional |
| Trial spawners and vaults | Excluded |
| Structure and jigsaw blocks | Excluded |

## 7. World simulation

| Feature | Progress |
| --- | --- |
| Game ticking | Done — 20 TPS simulation; 120 Hz deferred to future performance work. |
| Scheduled block ticks | Partial — delayed sand/gravel falls and stack support changes. |
| Scheduled fluid ticks | Conditional |
| Random ticks | Conditional |
| Block-entity ticking | Not started |
| Entity ticking | Partial — players and falling sand/gravel. |
| Fluid simulation | Conditional |
| Falling blocks | Partial — sand, red sand and gravel fall and settle on supported static terrain; unsuccessful landings discard with drops disabled. Contextual geometry and special entity motion media are unavailable. |
| Fire | Conditional |
| Explosions | Conditional |
| Lighting | Not started |
| Plant growth and spread | Conditional |
| Decay and material changes | Conditional |
| Vibration events | Excluded |
| Area effects | Not started |

## 8. Redstone and automation

| Feature | Progress |
| --- | --- |
| Power sources | Excluded |
| Signal propagation | Excluded |
| Repeaters | Excluded |
| Comparators | Excluded |
| Observers | Excluded |
| Pistons | Excluded |
| Powered mechanisms | Excluded |
| Note blocks | Not started |
| Dispensers | Excluded |
| Droppers | Excluded |
| Hoppers | Excluded |
| Automated crafting | Excluded |
| TNT ignition | Conditional |
| Redstone ordering behavior | Excluded |

## 9. Chunks, generation, and dimensions

| Feature | Progress |
| --- | --- |
| Chunk loading and unloading | Partial — startup map import; no arena unloading. |
| Terrain generation | Excluded |
| Biome generation | Excluded |
| Structure generation | Excluded |
| Features and decoration | Excluded |
| World presets | Excluded |
| Chunk streaming | Partial — imported terrain plus current edits, including reloads; no block entities or real lighting. |
| Chunk batch feedback | Done |
| View distance | Done |
| Simulation distance | Not started |
| Chunk loading tickets | Not started |
| Chunk simulation levels | Not started |
| Chunk and biome updates | Not started |
| Built-in dimensions | Partial — definitions available; Overworld lobby only. |
| Custom dimensions | Conditional |
| Portals and gateways | Conditional |
| World borders | Not started |
| World height and limits | Partial — Overworld bounds and position validation. |

## 10. Environment and world rules

| Feature | Progress |
| --- | --- |
| World clocks | Not started |
| Day and night | Not started |
| Timelines | Conditional |
| Weather | Not started |
| Lightning | Conditional |
| Sleeping | Conditional |
| Difficulty | Conditional |
| Regional climate behavior | Conditional |
| Environment attributes | Not started |
| Dimension-specific hazards | Conditional |
| Game-rule state | Partial — player protection and fall/void damage rules. |

## 11. Entities and mobs

| Feature | Progress |
| --- | --- |
| Entity identity and lifecycle | Partial — human players only. |
| Entity tracking | Partial — players and falling sand/gravel, gated by delivered terrain. |
| Entity synchronization | Partial — players and falling sand/gravel. |
| Entity physics | Partial — player ground/fluid motion, swimming/crawling, climbing, cobwebs, powder snow and bubble columns, plus sand/gravel falls; other entity types unavailable. |
| Entity interaction | Not started |
| Living-entity attributes | Not started |
| Natural spawning | Excluded |
| Explicit spawning | Partial — human joins/respawns and sand/gravel falls from terrain edits. |
| Despawning and persistence | Not started |
| Mob decisions | Excluded |
| Pathfinding and navigation | Excluded |
| Perception and hostility | Excluded |
| Mob combat | Excluded |
| Mob world interaction | Excluded |
| Breeding and growth | Excluded |
| Taming and ownership | Excluded |
| Leashes and home areas | Excluded |
| Villager society | Excluded |
| Raids, patrols, and sieges | Excluded |
| Boss encounters | Excluded |
| Mob transformations | Excluded |
| Utility mob behavior | Excluded |
| Decorative entities | Not started |
| Display and interaction entities | Not started |
| Mannequins | Not started |

## 12. Vehicles and transport

| Feature | Progress |
| --- | --- |
| Mounting and passengers | Not started |
| Riding input | Conditional |
| Boats and chest boats | Conditional |
| Minecarts and rails | Conditional |
| Land mounts | Conditional |
| Mount equipment and storage | Conditional |
| Flying mounts | Conditional |
| Underwater mounts | Conditional |
| Elytra gliding | Conditional |
| Propelled movement | Conditional |
| Ender pearls and chorus fruit | Not started |
| Sitting on cushions | Not started |

## 13. Presentation and communication

| Feature | Progress |
| --- | --- |
| Player chat | Not started |
| Secure chat | Not started |
| System messages | Not started |
| Direct and team messages | Not started |
| Rich text | Not started |
| Player-list entries | Done |
| Scoreboards | Not started |
| Teams | Not started |
| Boss bars | Not started |
| Titles and action bar | Not started |
| Player HUD state | Partial — health and hotbar; food stays fixed. |
| Locator bar and waypoints | Not started |
| Sounds | Partial — positioned and entity sounds for nearby players, with combat presets; observers hear player hurt sounds. |
| Particles | Partial — option-free particles for nearby players, with crit, hit, damage-indicator and sweep presets. |
| World and entity events | Partial — player damage, death, swings and critical-hit animations, shown to other players. |
| Maps and markers | Conditional |
| Camera targets | Not started |
| Dialogs | Not started |
| Pause-menu content | Not started |
| Server-controlled post effects | Not started |

## 14. Progression, statistics, and rewards

| Feature | Progress |
| --- | --- |
| Experience | Not started |
| Advancement criteria | Not started |
| Advancement presentation | Not started |
| Advancement rewards | Not started |
| Statistics | Not started |
| Recipe discovery | Conditional |
| Loot generation | Not started |
| Random sequences | Not started |

## 15. Commands and customization

| Feature | Progress |
| --- | --- |
| Command parsing and execution | Partial — `/gamemode <mode> [player]` accepts four mode names and numeric IDs 0–3; mode survives respawn. `/tp <player>`, `/tp <target> <player>` and `/tp [target] <x> <y> <z>` support world coordinates and issuer-relative `~` offsets, with validated destinations and readable success/error feedback. |
| Command suggestions | Partial — native `/gamemode` and `/tp` discovery, mode completion, online-player names and world coordinates. |
| Command permissions | Not started — `/gamemode` and `/tp` are available to all connected players. |
| Selectors and execution contexts | Excluded |
| World and entity manipulation | Partial — player teleport, health, inventory and game-owned block changes. |
| Data inspection and modification | Not started |
| Command storage | Excluded |
| Scoreboard logic and triggers | Not started |
| Functions | Excluded |
| Function scheduling | Not started |
| Command blocks | Excluded |
| Data-pack management | Excluded |
| Data-driven registries | Done — fixed 26.3 definitions. |
| Tags | Partial — client synchronization; no gameplay lookup. |
| Predicates and item modifiers | Not started |
| Game-rule commands | Not started |
| Feature flags and experiments | Done — stable feature set only. |
| Structure placement tools | Not started |
| Game tests | Done — focused gameplay checks. |
| Data reports | Not started |

## 16. Persistence and world data

| Feature | Progress |
| --- | --- |
| World metadata | Partial — imported spawn only. |
| Chunk storage | Partial — map import; no saving. |
| Entity storage | Not started |
| Points of interest | Excluded |
| Player state | Not started |
| Player progression | Not started |
| Shared saved data | Not started |
| Clocks and schedules | Not started |
| Autosaving | Not started |
| Explicit saving | Not started |
| Data-version upgrades | Not started |
| Session locking and load failures | Partial — map load errors; no writable-storage locking. |

## 17. Administration and operations

| Feature | Progress |
| --- | --- |
| Startup configuration | Not started |
| Initial server setup | Partial — startup/map validation; no setup configuration. |
| Player capacity | Done — human admission limits and online counts. |
| Operators | Not started |
| Allowlist | Not started |
| Player and IP bans | Not started |
| Kicking | Not started |
| Spawn protection | Partial — placement and non-Creative digging disabled; Creative breaking allowed; no editable-region permissions. |
| Server code of conduct | Not started |
| Console | Not started |
| RCON | Conditional |
| Query service | Conditional |
| Server management API | Conditional |
| Management notifications | Conditional |
| Connection and packet limits | Partial — message/admission bounds and timeouts; no rate policies. |
| Watchdog | Not started |
| Tick controls | Conditional |
| Profiling and metrics | Partial — gameplay measurements and diagnostics. |
| Empty-server pause | Not started |
| Logging and crash reports | Partial — opt-in connection/gameplay diagnostics; no crash reports. |
| Graceful shutdown | Not started |

## 18. Match lifecycle, matchmaking, and game rules

| Feature | Progress |
| --- | --- |
| Game definitions and native hooks | Not started |
| Match state machine | Not started |
| Player queues and match assignment | Not started |
| Bot-filled starts and replacement policy | Not started |
| Native team membership and objectives | Not started |
| Loadouts and spawn assignments | Partial — loadouts, item grants and player respawn points; no matches. |
| Round timers and scheduled events | Not started |
| Disconnect and rejoin policy | Not started |
| Party queues and skill-based matchmaking | Conditional |

## 19. Arenas, templates, and content boundaries

| Feature | Progress |
| --- | --- |
| Arena templates and map metadata | Partial — terrain import; no arena templates or objective metadata. |
| Independent mutable match worlds | Not started |
| Reset and teardown | Not started |
| Arena audience isolation | Not started |
| Supported-content validation | Partial — unknown states, unsupported movement contacts and invalid destinations rejected; no complete arena-content validation. |
| Map preparation and navigation anchors | Not started |
| Live creative arena editor | Conditional |

## 20. Native bots and humanlike play

Design expectations and layered architecture: [bots.md](bots.md).

| Feature | Progress |
| --- | --- |
| Native bot participants | Partial — `HALCYON_BOTS=<n>` spawns lobby bots through the human session path, deciding once per simulation step; they climb toward a goal (`HALCYON_BOT_GOAL` or the highest standable cell near spawn). No matches yet. |
| Bot identity and visual representation | Not started |
| Shared action/rule path | Partial — bots act only through owner-driven movement inputs, teleport confirmation and respawn, like human sessions. |
| Perception and short-term memory | Not started |
| Humanlike decisions and reaction timing | Partial — eased, rate-limited aiming with tremor, committed aim error, late key presses and hesitation before hard jumps; no perception or reaction model yet. |
| Bot combat and item use | Not started |
| Bot navigation and local steering | Partial — lazily validated walk/drop/jump/ladder/vine/slime-bounce graph, gold-plate checkpoints, budgeted A*, closed-loop edge execution, swim-to-shore recovery and checkpoint returns when trapped; no swimming routes, momentum chaining or neo jumps. |
| Navigation invalidation after edits | Partial — arena column revisions drop cached cells and edges nearby; unit-tested, not exercised by a live minigame. |
| Team coordination and game objectives | Not started |
| Difficulty and behavior profiles | Partial — a fixed per-bot level (`HALCYON_BOT_LEVEL`) sets aim, timing, hesitation and risk aversion; no traits or in-match state yet. |
| Reproducible bot scenarios | Partial — seeded headless bots on synthetic courses (unit tests) and on real maps (`spire_climb_scenario`, survey tools). |
| Learned behavior or training pipeline | Conditional |

## 21. Player records and game presentation

| Feature | Progress |
| --- | --- |
| Durable player identity and preferences | Not started |
| Match results and progression records | Not started |
| Game selection and lobby flow | Not started |
| Spectator and results flow | Not started |
| Rankings, economy, and unlock systems | Conditional |

## 22. Scheduling and operational foundations

| Feature | Progress |
| --- | --- |
| Authoritative simulation ownership | Partial — shared lobby; no independent matches. |
| Multi-core execution | Not started |
| Separate decision and simulation scheduling | Not started |
| Bounded queues and admission | Done |
| Reusable data and work buffers | Done |
| Off-executor durable I/O | Not started |
| Focused diagnostics and measurement | Done — connection/gameplay diagnostics and benchmarks. |
