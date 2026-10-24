# Halcyon feature progress

Last reviewed: 2026-10-23. Target: Java Edition 26.3.

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
| Disconnect handling | Partial — session cleanup; games see departures; no rejoin. |
| Incoming server transfers | Conditional |

## 2. Players, movement, and lifecycle

| Feature | Progress |
| --- | --- |
| Survival mode | Partial — player lifecycle and damage rules; no survival loop. |
| Creative mode | Partial — mode switching, flight, invulnerability, supported block placement, instant breaking with no drops and basic self-only `/give`; Creative inventory acquisition remains unavailable. |
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
| Melee targeting | Done — players attack other players with vanilla reach, and bots can be hit and knocked back; other entity types are not attackable yet. |
| Attack cooldowns | Done — 26.3 weapon attack speed and damage, charge reset by attacks, swings and switching items. Maces and spears cannot attack. |
| Critical hits | Done — falling, airborne, not climbing, out of water and not sprinting; status effects that block them are unavailable. |
| Sweeping attacks | Partial — grounded, slow sword attacks sweep nearby players; no Sweeping Edge. |
| Mace smashes | Conditional |
| Spear attacks | Conditional |
| Knockback | Partial — hurt, sprint and sweep knockback with armor knockback resistance; no Knockback enchantment. |
| Shields and blocking | Not started |
| Ranged weapon use | Not started |
| Projectile motion and hits | Not started |
| Projectile ownership | Not started |
| Damage types and attribution | Partial — fall, void, player-attack and game damage with optional source. |
| Damage cooldowns | Done — shared damage immunity window. |
| Armor and protection | Partial — worn armor points and toughness reduce player attacks; no enchantments or armor wear. |
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
| Player-versus-player rules | Partial — per-player PvP rule, off in the protected lobby (`HALCYON_PVP=1` opens it); no team rules. |

## 4. Items and inventory

| Feature | Progress |
| --- | --- |
| Item stacks | Done |
| Item components | Partial — defaults plus damage, unbreakable, name, lore, dye and glint overrides. |
| Player inventory | Done — storage, hotbar, armor, offhand and cursor. |
| Equipment changes | Partial — held items, armor and offhand synchronized; weapon and armor values apply to melee, other attribute effects do not. |
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
| Creative item acquisition | Partial — basic self-only `/give <item> [count]` for supported blocks, default mining tools and vanilla music discs; no Creative inventory acquisition or component syntax. |

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
| Block placement | Partial — an explicit arena-building catalog includes cubes, wood pillars, slabs/stairs, fences/walls, manual openings, signs and jukeboxes, with orientation, source-water logging, slab merging, support and occupancy checks. Creative lobby placement is enabled; Survival placement requires a Rust game rule. Other placement is rejected. |
| Block breaking | Partial — instant Creative breaking and game-enabled Survival mining of supported blocks using vanilla hardness, default tool speeds and ground/underwater penalties; paired-block removal, water preservation and no drops. Durability, enchantments and status modifiers remain unavailable. |
| Break feedback | Done — observer crack stages, cancellation cleanup, break sounds and particles. |
| Block drops | Conditional |
| Interaction validation | Partial — supported placement, mining, openings and jukebox use check mode/rules, reach, player life and authoritative held items; placement also checks support and occupancy. Held-slot changes and predictions retain their received order; unsupported predictions are corrected. |
| Predicted-action correction | Partial — authoritative terrain and inventory precede sequence acknowledgements, including overflow and stale-life rejection; collision corrections resend nearby terrain. Unsupported mechanics remain disabled. |
| Neighbor and shape updates | Partial — supported stair shapes, fence/wall connections, gate wall state, door/sign support and sand/gravel support changes, including falling-block starts and settlement. Oversized update chains are rejected; no general block updates. |
| Block entities | Partial — imported and game-authored signs and jukeboxes, reliable updates, replacement/removal and chunk reloads; other block-entity types remain unavailable. |
| Doors, trapdoors, and gates | Partial — hand-operated openings, supported placement, paired doors, observer sounds and explicit Rust game control; waterlogged trapdoors retain static water without flow. Redstone operation remains excluded. |
| Beds and respawn anchors | Conditional |
| Straw beds | Conditional |
| Signs | Partial — static standing, wall and hanging signs display front/back text, supported formatting, color and glow; Rust game APIs can change text. Player editing, dye/glow/wax interactions and executable text actions are disabled. |
| Shelves and storage interactions | Conditional |
| Jukeboxes | Done — manual vanilla-disc insertion, playback, note particles, natural completion, ejection and Rust game control; removed discs are discarded. Imported discs start stopped. Chunk reloads restart an active track from its beginning; automation remains excluded. |
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
| Block-entity ticking | Partial — active jukebox playback duration and note particles; no general block-entity ticking. |
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
| Chunk streaming | Partial — imported terrain, current edits and supported sign/jukebox data, including reloads; no real lighting. |
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
| Game-rule state | Partial — player protection, PvP and fall/void damage rules. |

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
| Scoreboards | Partial — game APIs provide shared and personal sidebars, below-name and Tab scores, formatted titles/labels and custom number formats. FFA has a personal status sidebar. No teams, Tab header/footer, scoreboard commands or persistence. |
| Teams | Not started |
| Boss bars | Not started |
| Titles and action bar | Not started |
| Player HUD state | Partial — health and hotbar; food stays fixed. |
| Locator bar and waypoints | Not started |
| Sounds | Partial — positioned and entity sounds for nearby players, including melee attacks, opening/placement/break feedback and jukebox playback; observers hear player hurt sounds. |
| Particles | Partial — option-free particles for nearby players, including melee damage-indicator and sweep particles, jukebox notes and block-break events. |
| World and entity events | Partial — player damage, death, swings and critical-hit animations, block destruction stages, break events and jukebox playback. |
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
| Command parsing and execution | Partial — `/gamemode <mode> [player]` accepts names or numeric IDs; `/tp` accepts players and world coordinates with issuer-relative `~` offsets. Basic self-only Creative `/give <item> [count]` grants supported plain items and reports inventory remainders. Commands provide readable success/error feedback. |
| Command suggestions | Partial — native `/gamemode`, `/tp` and `/give` discovery, mode completion, online-player names, world coordinates and supported item identifiers. |
| Command permissions | Partial — `/give` requires Creative mode; `/gamemode` and `/tp` remain available to all connected players. No operator system. |
| Selectors and execution contexts | Excluded |
| World and entity manipulation | Partial — player teleport, health, inventory, terrain changes, static sign text, manual opening control and jukebox playback through Rust game APIs. |
| Data inspection and modification | Not started |
| Command storage | Excluded |
| Scoreboard logic and triggers | Partial — games explicitly set objective scores and display assignments; no automatic vanilla criteria, operations, triggers or command interface. |
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
| Spawn protection | Partial — Creative placement/breaking enabled; Survival editing requires explicit Rust game rules and stays disabled in the lobby. No editable-region permissions. |
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

Game-facing capabilities and how to write a minigame: [minigames.md](minigames.md).

| Feature | Progress |
| --- | --- |
| Game definitions and native hooks | Partial — games attach to an instance with join, death, respawn, departure and per-step hooks; built-in hub and free-for-all. |
| Match state machine | Partial — a shared match lifecycle gathers players, counts down, runs the round, announces winners and returns everyone to the hub; free-for-all uses it. No joining a running match. |
| Player queues and match assignment | Partial — `/play <game>` joins an open instance of that game or starts one; `/hub` returns. No queues or parties. |
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
| Independent mutable match worlds | Partial — every instance has its own terrain overlay on a shared map; any map in `worlds/` can host a game. |
| Reset and teardown | Partial — finished or abandoned game instances stop once empty; no reset in place. |
| Arena audience isolation | Partial — players see only their instance's players, terrain and effects. |
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
| Bot navigation and local steering | Partial — lazily validated walk/drop/jump/ladder/vine/slime-bounce graph, gold-plate checkpoints, budgeted A* with search-ahead and exploring while a route is unknown, closed-loop edge execution, swim-to-shore recovery and checkpoint returns when trapped; no swimming routes, momentum chaining or neo jumps. |
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
| Game selection and lobby flow | Partial — players join the fullest hub with room, which opens another hub when full, and pick a game with `/play`. |
| Spectator and results flow | Not started |
| Rankings, economy, and unlock systems | Conditional |

## 22. Scheduling and operational foundations

| Feature | Progress |
| --- | --- |
| Authoritative simulation ownership | Partial — one owner per instance; instances spread over executors. |
| Multi-core execution | Partial — one executor per core, players fill one hub across cores before another opens; new games fill or start on the least-loaded core and players move between cores with their connection. No load rebalancing of running games. |
| Separate decision and simulation scheduling | Not started |
| Bounded queues and admission | Done |
| Reusable data and work buffers | Done |
| Off-executor durable I/O | Not started |
| Focused diagnostics and measurement | Done — connection/gameplay diagnostics and benchmarks. |
