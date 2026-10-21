# Halcyon server feature assessment

This document evaluates every entry in the [vanilla feature inventory](vanilla-server-features.md), preserving its 295 feature names, ordering, and 17 categories. Sections 18–22 add capabilities specific to Halcyon. The baseline is Java Edition **26.3**. These are proposed product and engineering choices, not a list of completed features or a commitment to implement everything marked Include immediately.

The intended platform supports combat, building, movement, and party games, including games populated by humanlike bots. Maps are imported; game logic is written in Rust. Running existing vanilla command-block or data-pack minigames is not a compatibility goal. No particular minigame lineup or numerical population target has been selected.

**Redstone is explicitly out of scope and will not be reimplemented.** This excludes power generation and propagation, circuit components, powered block behavior, pistons, and vanilla redstone/automation machinery. Rust game handlers may react directly to player interactions or control arena state. Manual block interactions, audio, and non-redstone TNT mechanics are assessed separately; supporting them does not imply circuit support.

**Vanilla world generation is explicitly out of scope and will not be reimplemented.** All arena terrain, biomes, structures, and decoration come from prepared maps. Importing, instancing, editing, and resetting those maps remain supported capabilities; missing terrain is not generated at runtime.

**Vanilla mob AI is explicitly out of scope and will not be reimplemented.** This excludes vanilla mob brains, goals, senses, hostility, pathfinding, autonomous combat, and specialized autonomous behaviors. Entity representation, physics, decoration, and any explicitly supported player-controlled riding are separate capabilities. Halcyon's humanlike player bots retain their own native decisions, perception, navigation, and combat actions.

**Data packs and command programming are explicitly out of scope.** Commands should be short, direct actions that players or staff would reasonably type in chat. Support simple names, choices, numbers/coordinates, and ordinary message or reason text, with permissions, help, and completion. Exclude nested/chained execution, selector expressions, command functions, macros, command blocks, expressions, and inline JSON/NBT/component editing. This is a usability boundary, not an arbitrary character limit. Console or remote access must not introduce a second, more elaborate command language.

Versioned registry/tag data, item codecs, and definitions required for client compatibility remain necessary. They do not imply support for loading or executing data packs; selected gameplay rules are implemented directly in Rust.

## How to read the assessment

The main tradeoff is **usefulness versus added code and maintenance**. A short, reusable capability can justify inclusion before a specific game needs it. A substantial subsystem needs a convincing platform requirement or an actual game that will use it. Runtime efficiency remains a project constraint, but the size ratings below do not measure CPU, memory, latency, or population capacity.

| Recommendation | Meaning |
| --- | --- |
| Include | Worth having as shared infrastructure or a small, reasonably useful capability. Existing implementations should be retained and completed where necessary. |
| Conditional | Add only when the stated game or operational requirement exists; do not implement solely to fill the vanilla checklist. |
| Replace | Supply the stated Halcyon capability instead of implementing the entire vanilla facility. The replacement does not promise compatibility with that facility. |
| Omit | The current platform assumptions do not justify this capability, or it is explicitly out of scope. Explicit exclusions remain excluded unless that scope decision changes. |

| Code size | Meaning |
| --- | --- |
| Small | A localized codec, state change, calculation, or helper with a limited maintenance surface. |
| Medium | A contained feature with several operations, lifecycle rules, or integration points. |
| Large | A substantial subsystem with many behavioral cases, supporting state, or interactions. |
| Very large | Several coupled subsystems or a broad compatibility surface that would materially expand the server. |

Size is an engineering estimate of the handwritten implementation scope, including behavior and necessary validation, not an estimate of remaining work or delivery time. Existing support is identified separately below. Generated tables and imported assets do not count as handwritten code, although their consumers and validation do. Small shared features can become cheap additions once their foundations exist; that does not make those foundations cheap.

Vanilla rows estimate faithful behavior for that feature family. Replacement rows give **Halcyon / vanilla** sizes in that order. Shared foundations are assessed in their own rows and are not counted repeatedly; important dependencies are named in the rationale. Sizes must not be added together as if they were independent estimates. Native-only rows estimate the proposed Halcyon capability.

Supported gameplay mechanics should behave exactly like the target vanilla version, including relevant interactions and edge cases, within the explicit exclusions above. Selecting a smaller set of items, blocks, or game mechanics is allowed; silently approximating an enabled mechanic is not. Unsupported interactions must be unavailable in the game's content or explicitly disabled by its rules. A custom game objective, shop, spawn rule, or victory condition is an intentional Halcyon rule, not a claim of vanilla parity. A block or entity can be present visually without exposing all its interactions. Exactness for supported mechanics does not require adding excluded redstone, world generation, mob AI, data packs, or command programming.

## Current implementation snapshot

This assessment is based on repository inspection on 2026-10-21, not a new live-client compatibility test.

- **Working foundations:** online login, encryption, and profile handling are in [login](../crates/halcyon-server/src/states/login.rs) and [transport](../crates/halcyon-server/src/transport.rs). [Connection lifecycle](../crates/halcyon-server/src/lifecycle.rs) and the [client loop](../crates/halcyon-server/src/client.rs) provide deadlines, keepalives, bounded reads, and cooperative yields.
- **Read-only world delivery:** the [world loader](../crates/halcyon-world/src/loader.rs) imports terrain into an [immutable world](../crates/halcyon-world/src/lib.rs). [Wire caches](../crates/halcyon-server/src/cache.rs) share configuration and chunk data; [chunk scheduling](../crates/halcyon-server/src/chunks.rs) handles residency, view distance, batching, and feedback. Sent chunks currently omit block entities and use a fullbright lighting suffix; this is not lighting simulation.
- **Partial player support:** the [Play handler](../crates/halcyon-server/src/states/play.rs) receives position and rotation, performs finite-value checks and coordinate clamping, handles initial teleport confirmation, and updates chunk residency. It does not validate terrain collision or broadcast movement between players. Initial health, abilities, and skin packets are not health simulation, complete game-mode support, or general entity tracking.
- **Partial protocol and configuration support:** [configuration](../crates/halcyon-server/src/states/config.rs) enters a basic Play session. [Status](../crates/halcyon-server/src/states/status.rs) answers pings but advertises hardcoded population values. The [packet definitions](../crates/mc-protocol/src/packets/mod.rs) include a compression field and packet types without implemented compressed framing. Recognizing an incoming transfer intent is not a configured transfer service or admission policy.
- **Absent platform systems:** [server state](../crates/halcyon-server/src/server.rs) currently contains lobby data, keys, caches, and diagnostics. There is no shared player population, authoritative gameplay simulation, mutable arena state, inventory/combat subsystem, bot implementation, matchmaking, or match lifecycle. The [entry point](../crates/halcyon-server/src/main.rs) runs one executor on a fixed core; this is the starting implementation, not the target scaling architecture.

The recommendations and size bands are judgments. Vanilla behavior references are collected in the companion inventory; direct official references below explain particularly important version-sensitive boundaries.

## 1. Connections and compatibility

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Server-list status | Include | Small | Required public entry point; reuse the responder and replace hardcoded counts with actual server state. |
| Status latency measurement | Include | Small | Already supported and useful to every human player; little ongoing code. |
| Protocol negotiation | Include | Small | Clearly admit the supported version and intents; broad multi-version translation is a separate, unjustified project. |
| Online authentication | Include | Medium | Existing identity foundation for human accounts; retain verification and failure handling. Bots do not need client login sessions. |
| Offline-mode identity | Omit | Small | Another identity/admission path has little value for the intended public service; native bots do not require it. |
| Profile properties | Include | Small | Existing skin support is useful presentation infrastructure once authenticated identities exist. |
| Encrypted transport | Include | Medium | Already implemented and part of authenticated human connections; retain correct partial-write and cipher state handling. |
| Packet compression | Include | Medium | A bounded framing/codec addition with broad value for terrain delivery; implement actual framing before negotiating it. |
| Configuration phase | Include | Medium | Required client entry path; shared versioned registry and tag payloads already provide much of the foundation. |
| Reconfiguration | Conditional | Medium | Add when synchronized configuration must change without reconnecting; ordinary match changes need not require live registry reloads. |
| Client preferences | Include | Small | Locale, view distance, and skin preferences are inexpensive to retain and useful across games. |
| Resource-pack delivery | Include | Medium | Pack requests and result tracking support themed maps and UI without implementing a rendering engine. |
| Keepalives and inactivity | Include | Small | Retain existing liveness handling; add any inactivity policy as an explicit server setting. |
| Disconnect handling | Include | Medium | Needed for cleanup, readable failures, and match membership; grows beyond the current per-connection scope once shared players exist. |
| Incoming server transfers | Conditional | Small | Add an explicit acceptance policy if an external entry server redirects players; intent decoding alone is not that policy. |

The [official networking notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-20-5) distinguish transfer acceptance from custom-server transfer/cookie facilities. Neither requires building a proxy network into Halcyon.

## 2. Players, movement, and lifecycle

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Survival mode | Replace | Medium / Very large | Compose match rules from the supported survival mechanics below; reproducing a complete open-ended survival experience is unnecessary. |
| Creative mode | Conditional | Large | Implement faithful creative interactions if players or staff must build inside Halcyon. Imported maps avoid requiring a live editor. |
| Adventure mode | Include | Medium | A natural fit for protected arenas and lobbies; item-based exceptions depend on item components and interaction validation. |
| Spectator mode | Include | Large | Essential for eliminated players and observation; include camera, interaction, visibility, and transition rules, not just a mode flag. |
| Hardcore rules | Omit | Small | One-life matches belong to match elimination rules; world-level permanent death adds a second lifecycle model without a distinct need. |
| Ground movement | Include | Large | Core to every game and bots; existing position storage lacks authoritative checks and observer synchronization. |
| Collision and stepping | Include | Large | Essential to fair movement and navigation; collision shapes, stepping, and prediction compatibility are substantial shared code. |
| Sprinting and sneaking | Include | Medium | Common gameplay inputs with movement, pose, and visibility effects; worthwhile once the movement foundation exists. |
| Jumping and falling | Include | Medium | Required for ordinary traversal, parkour, and combat; depends on collision, motion, and fall-damage state. |
| Swimming and crawling | Include | Medium | Basic traversal can occur on many imported maps; reuse movement while preserving pose and hitbox transitions. |
| Climbing | Include | Medium | Ladders and vines offer reusable map design options; implement their movement interactions on the shared collision foundation. |
| Movement surfaces and media | Conditional | Large | Implement each surface family when enabled maps use it; the complete ice/slime/honey/fluid/web behavior family is not one small multiplier. |
| Flight permissions | Include | Medium | Needed for spectators and staff; permissions and movement validation must agree about authorized flight. |
| Movement abilities and attributes | Include | Large | Shared movement and combat require supported vanilla attributes and modifiers; importing values alone does not execute their behavior. |
| Teleportation | Include | Medium | Match entry, resets, and corrections need reusable teleport state beyond the initial spawn teleport. |
| Initial spawning | Replace | Small / Medium | Use map-defined safe lobby and team spawn points; omit vanilla's general world-spawn search. |
| Personal respawn points | Replace | Small / Medium | Store match/team respawn assignments; add actual bed/anchor respawn semantics only if a game exposes them. |
| Death processing | Include | Large | Shared combat and elimination foundation; coordinate drops, attribution, scores, cleanup, and match callbacks. |
| Respawning | Include | Medium | Required between lives and rounds; build on complete player/death state and synchronize all reset properties. |
| Dimension transitions | Include | Medium | Useful for moving between imported arenas with different dimension definitions; depends on player, chunk, and entity cleanup. |
| Player poses and actions | Include | Medium | Humans and bots need correct visible actions; reuse entity metadata and equipment synchronization. |
| End completion flow | Omit | Medium | Credits and vanilla End completion have little use in round-based games; ordinary result screens use existing presentation primitives. |

Exact movement is a major foundation, not a cheap consequence of receiving client coordinates. Bot navigation can choose different routes, but the resulting movement must use the same supported physical rules.

## 3. Combat and survival

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Melee targeting | Include | Large | Core PvP behavior; requires entity hitboxes, reach checks, and authoritative target state. |
| Attack cooldowns | Include | Small | A compact rule once attack state and attributes exist; preserve vanilla timing instead of adding custom combat tuning. |
| Critical hits | Include | Small | Useful and relatively localized after movement and attack state are trustworthy. |
| Sweeping attacks | Include | Medium | Common sword behavior; nearby-target selection adds cases but reuses combat and spatial queries. |
| Mace smashes | Conditional | Medium | Add when a game's loadouts include maces; fall state, damage, and applicable enchantment interactions must already exist. |
| Spear attacks | Conditional | Large | Add for spear-enabled loadouts; reach, charge stages, relative motion, and dismounting are substantial compared with ordinary attack cooldowns. |
| Knockback | Include | Medium | Central to arena combat; depends on shared motion, collision, and resistance calculations. |
| Shields and blocking | Include | Medium | Broad combat value after item-use and damage pipelines exist; include directional and disabling rules. |
| Ranged weapon use | Include | Medium | Bows and crossbows enable common game types; requires inventory, item-use state, and projectile support. |
| Projectile motion and hits | Include | Large | Reusable for combat and thrown utility items; collision, timing, and type-specific behavior require a real subsystem. |
| Projectile ownership | Include | Medium | Fair attribution and pickup rules matter; implement alongside each supported projectile type. |
| Damage types and attribution | Include | Large | Shared source-of-truth for combat, hazards, deaths, and scoring; effects need consistent source handling. |
| Damage cooldowns | Include | Small | Short shared damage-state logic with significant effect on vanilla combat feel. |
| Armor and protection | Include | Medium | Common loadout behavior; requires attributes, supported enchantments, and damage-specific exceptions. |
| Health and absorption | Include | Small | Core state and synchronization once damage exists; current initial health packets do not implement it. |
| Healing and regeneration | Include | Medium | Reusable survival/combat rules; account for food, effects, and game-rule settings rather than approximating regeneration. |
| Hunger, saturation, and exhaustion | Include | Medium | Supports ordinary vanilla combat and movement; games may deliberately freeze food through their rules. |
| Eating and drinking | Include | Medium | Common utility items; depends on item components, use duration, inventory, and effects. |
| Status effects | Include | Large | Strong cross-game value, but the complete effect family has many interactions; implement exact supported effects and gate the rest. |
| Splash and lingering effects | Conditional | Medium | Add when potion-based loadouts are enabled; needs projectiles, effect application, and area queries. |
| Environmental damage | Include | Large | Void and fall damage are essential; enable additional hazard families with exact behavior when their map content is admitted. |
| Air and exposure state | Include | Medium | Reusable survival timers after hazards exist; required wherever water, fire, or freezing are enabled. |
| Death prevention | Include | Small | Totem handling is a useful localized addition after death, inventory, and effects are available. |
| Enchantment effects | Conditional | Very large | Implement exact selected enchantments directly in Rust when enabled equipment needs them; omit a general data-pack effect interpreter and the unused enchantment catalog. |
| Player-versus-player rules | Include | Small | Team damage and permitted attacks are small, essential policies on top of combat and team membership. |

For example, [vanilla spear behavior](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-11) includes motion-dependent effects and several charge stages. A small weapon packet is not a useful estimate of that mechanic's implementation size. Enchantments needed by included equipment become prerequisites, even though implementing the whole enchantment catalog is conditional.

## 4. Items and inventory

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Item stacks | Include | Medium | Shared foundation for loadouts, inventories, pickups, and rewards; compatibility includes stack properties, not only count and item ID. |
| Item components | Include | Large | Required for authentic equipment and custom presentation; many component schemas and consumers make this more than a generic NBT field. |
| Player inventory | Include | Large | Common to combat and building games; owns slots, cursor state, selected equipment, and lifecycle integration. |
| Equipment changes | Include | Medium | Required for visible loadouts and attribute updates; reuses inventory and entity synchronization. |
| Inventory clicks | Include | Large | Ordinary client interactions expose many click modes and slot rules; omitting edge cases produces incorrect item state. |
| Inventory reconciliation | Include | Medium | Required to correct prediction and prevent divergence; depends on the inventory model and supported container rules. |
| Container sessions | Include | Medium | Chests and reusable game menus justify a common open/close/access lifecycle. |
| Container properties | Include | Small | A reusable synchronization helper; specialized screens still require their own server behavior. |
| Item pickup | Include | Medium | Essential dropped loot behavior after inventory and spatial entity queries exist. |
| Item dropping | Include | Medium | Common player action and combat result; requires dropped-entity creation and inventory ownership changes. |
| Dropped-item lifecycle | Include | Medium | Needed to complete ordinary drops; timers, merging, pickup delay, and damage reuse entity infrastructure. |
| Durability and breakage | Include | Small | Compact equipment behavior with broad use after item components and relevant enchantment hooks exist. |
| Item-use cooldowns | Include | Small | A short timer/synchronization capability useful for many vanilla utility items. |
| Nested item storage | Conditional | Large | Add bundles or portable storage when a game uses them; nested stack handling and interactions expand inventory code. |
| Ender chest storage | Include | Small | Reuses container and inventory primitives; per-match private storage is plausibly useful even before a specific game needs it. |
| Books and written content | Conditional | Medium | Add editable books or lectern interaction for a game or authoring requirement; signs and dialogs already cover simpler instructions. |
| Fishing | Conditional | Large | Add for fishing games or rod-enabled combat; timers, loot, projectile state, and pulling are more than rendering a hook. |
| Navigation item targets | Include | Small | Target data is useful for compasses and map mechanics; keep matching vanilla item semantics for the chosen target type. |
| Creative item acquisition | Conditional | Medium | Add with an in-server creative builder workflow; avoid exposing it to ordinary game players. |

The [official item-component introduction](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-20-5) explains default and per-stack components. Wire representation, storage, and the gameplay consumers are separate pieces of code; passing through component data does not implement its effect.

## 5. Crafting, processing, and trading

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Grid crafting | Conditional | Large | Add when a chosen game allows player crafting; matching recipes and grid behavior require inventory and recipe data support. |
| Crafting outputs and remainders | Conditional | Medium | Required if grid crafting is enabled; recipe consumption and remainder handling cannot be skipped for otherwise supported recipes. |
| Recipe book interaction | Conditional | Medium | Add with crafting screens; client ingredient placement needs real inventory validation and recipe knowledge. |
| Furnace processing | Conditional | Medium | Worth adding for resource-processing games after inventories, recipes, block entities, and tick scheduling exist. |
| Specialized cooking | Conditional | Medium | Add selected cooker families when used by a game's content; furnace infrastructure makes some extensions modest. |
| Brewing | Conditional | Medium | Add if players brew potions during a match; preassigned potions only need consumption and effect behavior. |
| Enchanting table | Conditional | Large | Add if live enchantment choices are gameplay; preconfigured enchanted loadouts avoid offer-generation and workstation logic. |
| Anvil | Conditional | Large | Add for live equipment-combination games; cost, compatibility, naming, and repair interactions create substantial behavior. |
| Grindstone | Conditional | Medium | Add when equipment repair/disenchanting is used; reuses components, enchantments, recipes, and screen handling. |
| Smithing | Conditional | Medium | Add when a game includes live upgrades or trim selection; static equipment does not require the workstation. |
| Stonecutting | Conditional | Small | A modest recipe-selection extension once recipe and container foundations exist; add with a crafting/building game that uses it. |
| Loom | Conditional | Medium | Add for a banner-design or creative game; patterns, selection state, and item transformation have a limited general use. |
| Cartography table | Conditional | Medium | Add when players modify maps; displaying or issuing prepared maps does not require its screen logic. |
| Merchant transactions | Replace | Medium / Medium | Use a typed Rust shop with explicit offers and inventory validation for game purchases; add vanilla merchant semantics only if actual villager trading is exposed. |
| Trade economics | Omit | Large | Reputation, demand, levels, and restocking have little role in fixed game shops and require a broader villager model. |
| Bartering | Omit | Medium | Vanilla piglin behavior is excluded with mob AI; a game shop or explicit Rust reward exchange does not require it. |

These workstation features are not prerequisites for obtaining the same finished items. Imported maps and Rust loadouts can supply items directly while preserving their enabled gameplay behavior.

## 6. Blocks and interaction

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Block states | Include | Medium | Existing IDs support imported terrain; mutable state access and metadata are needed for collision and interactions. |
| Block placement | Include | Large | Essential for building games; exact supported blocks need placement, shape, permission, and inventory rules. |
| Block breaking | Include | Large | Required for editable arenas; tool suitability, mining progress, permissions, and resulting changes interact. |
| Break feedback | Include | Small | A useful observer packet/update path once authoritative breaking and entity audiences exist. |
| Block drops | Conditional | Large | Add exact drops for blocks that a game allows players to harvest; blanket vanilla loot evaluation adds a broader dependency. |
| Interaction validation | Include | Large | Central authority for block/item actions; requires reach, content support, permissions, and inventory state. |
| Predicted-action correction | Include | Medium | Essential for rejected edits and protected areas; shares action sequencing, inventory, and world synchronization. |
| Neighbor and shape updates | Include | Large | Required to make supported connected or dependent blocks behave correctly; update propagation is shared world code. |
| Block entities | Include | Large | A shared representation and synchronization lifecycle are justified; enable individual types only with their specific behavior. |
| Doors, trapdoors, and gates | Include | Medium | Common arena elements; support manual interaction for applicable types and explicit Rust game control. Redstone operation is excluded. |
| Beds and respawn anchors | Conditional | Medium | Add actual sleep/charge/respawn behavior only for games that expose it; a destructible objective can be a custom game rule. |
| Straw beds | Conditional | Small | A plausible small extension once sleeping and temporary block behavior exist; defer until a sleeping game justifies those larger foundations. |
| Signs | Include | Medium | Static instructions and supported text state are broadly useful; full edit, dye, wax, and text-action rules add lifecycle cases. |
| Shelves and storage interactions | Conditional | Medium | Add interactive storage types when a selected game uses them; decorative placement alone does not justify the interactions. |
| Jukeboxes | Include | Small | Useful manually operated lobby/map audio once item/block interaction exists; redstone output and automation are excluded. |
| Buckets and fluid containers | Conditional | Large | Add when editable-water/lava or bucket-based games exist; requires exact fluid and block/container interaction semantics. |
| Tool transformations | Conditional | Medium | Add transformations used by enabled tools and editable maps; ensure each supported transformation's drops and updates are correct. |
| Farming and harvesting | Conditional | Large | Add for a concrete farming/resource game; crop state, growth, tools, and produce substantially extend world behavior. |
| Archaeology | Omit | Large | Brushing, suspicious blocks, rewards, and progression offer little shared minigame value under current assumptions. |
| Beacons and conduits | Conditional | Large | Add for games using real multiblock effects; scripted capture-point buffs are custom game logic rather than vanilla beacon support. |
| Trial spawners and vaults | Omit | Large | These bring their own encounter, player-eligibility, and reward systems; use Halcyon's match and wave rules for custom encounters. |
| Structure and jigsaw blocks | Omit | Very large | Runtime vanilla structure authoring/assembly is unnecessary for imported arenas; template/reset support is assessed separately below. |

Conditional behavior is not activated merely because its block exists in an imported map. Map validation must distinguish decoration from interactions that the game permits.

## 7. World simulation

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Game ticking | Include | Medium | Authoritative gameplay needs a shared tick timeline; keep match timers distinct from connection deadlines. |
| Scheduled block ticks | Include | Medium | Reuse a queue for delayed supported non-redstone block behavior; scheduling does not imply circuit-component support. |
| Scheduled fluid ticks | Conditional | Small | Add when fluids are enabled; the queue can reuse scheduling, but fluid simulation is a separate Large dependency. |
| Random ticks | Conditional | Medium | Add when an enabled mechanic depends on vanilla random updates; the sampler does not implement crop growth or decay itself. |
| Block-entity ticking | Include | Medium | A shared lifecycle for active supported block entities; do not add all vanilla block-entity implementations at once. |
| Entity ticking | Include | Large | Required by players, bots, drops, and projectiles; shared motion and lifecycle are distinct from each type's behavior. |
| Fluid simulation | Conditional | Large | Justify with a water/lava gameplay requirement; static imported fluid appearance alone does not need dynamic flow. |
| Falling blocks | Conditional | Medium | Add when maps allow unsupported sand, gravel, anvils, or similar behavior; depends on entity physics and mutable terrain. |
| Fire | Conditional | Large | Add when ignition/spread is gameplay; a burn effect on an entity does not require world fire propagation. |
| Explosions | Conditional | Large | Add for TNT or explosive loadouts; entity exposure, impulses, block destruction, and drops require shared combat and world support. |
| Lighting | Include | Large | Correct editable maps justify this foundation. Importing correct precomputed light helps static maps but does not solve updates after light-changing edits. |
| Plant growth and spread | Conditional | Large | Add only growth families required by a farming/resource game; decorative vegetation can remain static when interaction is disabled. |
| Decay and material changes | Conditional | Large | Add when editable content exposes these behaviors; support requirements must account for ordinary actions such as breaking a tree's logs. |
| Vibration events | Omit | Large | Vanilla sensor power and warden perception belong to excluded redstone and mob AI; native game events do not require their vibration machinery. |
| Area effects | Include | Medium | Reusable eligibility and spatial-effect application support potions and other enabled effects; source-specific rules remain separate. |

Frozen map decoration is a deliberate content restriction, not a cheaper implementation of dynamic vanilla behavior. Before enabling a block edit, include the update, lighting, drop, and other mechanics that edit can expose.

## 8. Redstone and automation

Redstone and its automation devices are excluded, including for future minigames. The rows remain here for comparison with vanilla. Direct game inputs, manual audio, and non-redstone TNT activation below are independent capabilities.

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Power sources | Omit | Medium | No vanilla power emission or detection system. Rust handlers may consume direct use or region-entry events as custom game inputs. |
| Signal propagation | Omit | Large | Redstone networks are explicitly out of scope; imported maps must not rely on wiring. |
| Repeaters | Omit | Medium | Circuit components are excluded, regardless of their individual state-machine size. |
| Comparators | Omit | Medium | No redstone signal comparison or container signal readout system. |
| Observers | Omit | Medium | No block-change-to-redstone pulse machinery; native game events cover deliberate arena scripting. |
| Pistons | Omit | Large | No vanilla piston movement, sticky-block contraptions, or their ordering semantics. |
| Powered mechanisms | Omit | Medium | No redstone-driven block operation; Rust game rules can explicitly change supported arena state. |
| Note blocks | Include | Small | Support manual tuning and playback for map audio after block state and sound support exist; powered activation is excluded. |
| Dispensers | Omit | Large | No vanilla automated item-action devices; games invoke their own explicit actions. |
| Droppers | Omit | Small | No vanilla redstone-triggered ejection/transfer devices, even though the local addition could be small. |
| Hoppers | Omit | Medium | No vanilla inventory-transport automation; deliberate game resource transfers belong in Rust game logic. |
| Automated crafting | Omit | Medium | No crafter/redstone automation; any supported player crafting remains a separate capability. |
| TNT ignition | Conditional | Small | Add non-redstone activation for a TNT game, through supported direct interactions or explicit Rust rules; faithful explosions remain a Large dependency. |
| Redstone ordering behavior | Omit | Very large | No Java circuit-ordering compatibility, quasi-connectivity, or redstone update engine. |

Native Rust rules control scripted arena events directly. Redstone blocks may appear as inert decoration where permitted, but maps and loadouts must not expose working circuitry or automation. A game that needs a button-triggered event should bind the interaction directly to its handler rather than introduce a power network.

## 9. Chunks, generation, and dimensions

World generation is a firm exclusion. Dimension definitions and imported biome/terrain data remain useful, but no game or map dependency should trigger implementation of a vanilla generator.

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Chunk loading and unloading | Include | Medium | Import, retain, and release arena data according to actual use; extend existing startup loading with map lifetimes. |
| Terrain generation | Omit | Very large | Explicitly excluded: import prepared terrain instead of implementing vanilla noise generation or generating missing chunks. |
| Biome generation | Omit | Large | Explicitly excluded: preserve imported biome assignments and definitions without generating their spatial distribution. |
| Structure generation | Omit | Very large | Explicitly excluded: structures must already exist in prepared maps; no vanilla procedural placement or assembly. |
| Features and decoration | Omit | Very large | Explicitly excluded: import placed ores and vegetation; no vanilla procedural feature or decoration generation. |
| World presets | Omit | Medium | Generator presets are excluded with world generation; select prepared arena templates instead. |
| Chunk streaming | Include | Large | Existing read-only streaming is valuable; mutable data, block entities, correct light, and arena transitions expand it. |
| Chunk batch feedback | Include | Medium | Already has a bounded implementation; retain batch lifecycle and feedback validation for human clients. |
| View distance | Include | Small | Already configurable per connection within a cap; keep synchronized bounds and correct residency. |
| Simulation distance | Replace | Medium / Medium | Activate simulation by arena and game requirements; retain compatible client-visible settings without implementing unrelated distant survival ticking. |
| Chunk loading tickets | Replace | Medium / Large | Use explicit arena/chunk lifetime ownership. Add vanilla portal/pearl ticket semantics only when their gameplay is enabled. |
| Chunk simulation levels | Replace | Medium / Large | Use loaded, active, and suspended arena state; vanilla distance-derived block/entity levels are unnecessary for fixed match instances. |
| Chunk and biome updates | Include | Medium | Mutable maps require incremental block and relevant biome updates; shares the authoritative world and observer model. |
| Built-in dimensions | Include | Medium | Retain built-in definitions and allow imported Overworld/Nether/End arenas. Their enabled environmental mechanics remain separately assessed prerequisites. |
| Custom dimensions | Conditional | Medium | Add when an arena requires definitions beyond built-in environments; data definitions do not require vanilla world generation. |
| Portals and gateways | Conditional | Large | Add for actual portal gameplay; destination search/creation, cooldowns, transitions, and loading lifetimes exceed a teleport helper. |
| World borders | Include | Medium | Reusable arena boundaries and shrinking-zone games justify the state, interpolation, warnings, and damage rules. |
| World height and limits | Include | Small | Required to validate world accesses and edits; definitions already supply much of the needed information. |

Loading visible terrain, simulating its behavior, and generating new terrain are independent choices. Built-in dimension definitions are useful without reimplementing every activity normally found in those dimensions.

## 10. Environment and world rules

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| World clocks | Include | Medium | A reusable time/paused-state model supports arena time control and the target version's environment definitions. |
| Day and night | Include | Small | Time updates and controlled progression are broadly useful after clocks exist; dependent spawning is a separate feature. |
| Timelines | Conditional | Medium | Add when maps use data-driven environmental transitions; a fixed arena clock does not require a general timeline evaluator. |
| Weather | Include | Small | Fixed weather and transitions are useful low-code presentation controls; world reactions remain separate. |
| Lightning | Conditional | Medium | Add when storms have gameplay consequences; a visual effect alone does not implement entity transformations or ignition. |
| Sleeping | Conditional | Medium | Add for a game that actually advances time through player sleep; needs bed checks, eligible-player tracking, and clock integration. |
| Difficulty | Conditional | Medium | Add when enabled survival damage/hunger rules require it; mob AI remains excluded, and native player-bot difficulty is a separate setting. |
| Regional climate behavior | Conditional | Large | Add when dynamic snow, freezing, or precipitation effects matter to a game; static climate presentation is not dynamic simulation. |
| Environment attributes | Include | Large | Needed to interpret supported dimension/biome behavior correctly; preserve data-driven semantics rather than scattering contradictory hardcoded rules. |
| Dimension-specific hazards | Conditional | Medium | Add when enabled maps/items expose those hazards; dimension definitions alone must not silently enable unsupported bed or fluid interactions. |
| Game-rule state | Replace | Small / Small | Use typed match/environment settings and preserve supported vanilla rule semantics. Storing rule values is small; implementing their affected systems is assessed separately. |

The [26.1 release notes](https://feedback.minecraft.net/hc/en-us/articles/44551668333837-Minecraft-Java-Edition-26-1) describe world clocks and their timeline integration. Loading definition data is distinct from implementing the rules that consume it.

## 11. Entities and mobs

The shared entity system supports players, native player bots, gameplay objects, and decoration. It does not include vanilla mob AI, even for a future PvE game; custom game actors must use explicit native rules.

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Entity identity and lifecycle | Include | Large | Essential shared foundation for humans, bots, drops, and projectiles; includes ownership, spawning, removal, and cleanup. |
| Entity tracking | Include | Large | Required to show participants and game objects to the right human observers; arena isolation must be built into audience selection. |
| Entity synchronization | Include | Large | Motion, equipment, metadata, and lifecycle events are central shared code; a self-skin packet is not this subsystem. |
| Entity physics | Include | Large | Required for gameplay objects and bot physical behavior; implement exact rules for supported types and media. |
| Entity interaction | Include | Medium | Shared validation and dispatch for attacks, uses, and other enabled actions; type-specific rules remain separate. |
| Living-entity attributes | Include | Large | Players and combat entities need consistent modifiers and derived values; overlaps with the shared attribute foundation, not a second engine. |
| Natural spawning | Omit | Large | Match rules explicitly create participants and supported objects; vanilla mob populations and their autonomous behavior are excluded. |
| Explicit spawning | Replace | Medium / Large | Spawn supported entities through Rust game APIs. A universal summon/NBT interpreter and every spawn-egg behavior are unnecessary. |
| Despawning and persistence | Replace | Medium / Medium | Tie entities to match/map lifetimes and explicit persistence policy; omit survival distance-despawn rules for unexposed mobs. |
| Mob decisions | Omit | Very large | Vanilla brains, goals, memories, and activity systems are explicitly excluded. Native humanlike player-bot decisions are assessed in section 20. |
| Pathfinding and navigation | Omit | Large | No vanilla walking, swimming, or flying mob navigators; native player-bot arena navigation remains a separate required system. |
| Perception and hostility | Omit | Large | No vanilla mob senses, target selection, anger, or threat reactions; native player bots use their own perception and team rules. |
| Mob combat | Omit | Large | No autonomous vanilla enemy attacks or boss AI; human and native player-bot combat share the supported player mechanics. |
| Mob world interaction | Omit | Large | No autonomous mob pickup, griefing, or terrain-changing behavior; native game rules control deliberate arena changes. |
| Breeding and growth | Omit | Large | Lifecycle and offspring systems have little general arena value and bring mob-specific rules. |
| Taming and ownership | Omit | Medium | No vanilla pet/taming behavior or owner-driven AI; any native game ownership is explicit game state. |
| Leashes and home areas | Omit | Large | No vanilla mob-home or leash-following behavior; native player-bot navigation does not require these systems. |
| Villager society | Omit | Very large | Jobs, points of interest, schedules, reputation, and golem behavior are substantial code without a platform use case. |
| Raids, patrols, and sieges | Omit | Very large | Native round/wave rules can provide game encounters without cloning vanilla's world-event machinery. |
| Boss encounters | Omit | Very large | Vanilla dragon, wither, and other boss behavior is excluded with mob AI; any custom encounter must be native game logic. |
| Mob transformations | Omit | Large | Do not add vanilla mob lifecycle systems around excluded behaviors; explicit game-driven representation changes can use entity APIs. |
| Utility mob behavior | Omit | Large | Allay collection, golem sorting, bee ecology, and other autonomous vanilla behaviors are explicitly excluded. |
| Decorative entities | Include | Medium | Armor stands, paintings, and frames are useful map content; interaction and attachment rules reuse entity/block foundations. |
| Display and interaction entities | Include | Medium | High reuse for signs, menus, objectives, and NPC hit targets with a contained behavioral surface after tracking exists. |
| Mannequins | Include | Medium | Useful player-shaped lobby/game actors after living-entity and profile support exist; they do not supply player-bot decisions. |

The [official mannequin description](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-9) identifies a player avatar without a connected player, with living-entity state. Halcyon still needs its own bot control and player-like actions; choosing an entity representation does not provide them.

## 12. Vehicles and transport

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Mounting and passengers | Include | Medium | A reusable relationship and synchronization facility supports seating and future vehicles; physical vehicle rules are separate. |
| Riding input | Conditional | Medium | Add when a chosen game permits controllable vehicles; the input packets do not implement their movement. |
| Boats and chest boats | Conditional | Large | Add for boat races or transport gameplay; exact motion, collision, passengers, and storage require more than generic entity movement. |
| Minecarts and rails | Conditional | Large | Add for rail games; rail following and cart interactions are a specialized physics subsystem. |
| Land mounts | Conditional | Large | Add only rider-controlled mechanics for specific mount types required by a game; wandering, following, taming, and other mob AI remain excluded. |
| Mount equipment and storage | Conditional | Medium | Add the equipment/inventory rules required by supported mounts; shares existing item and container code. |
| Flying mounts | Conditional | Large | Add player-controlled happy ghast riding only for a selected game; autonomous roaming and other mob AI remain excluded. |
| Underwater mounts | Conditional | Large | Add player-controlled nautilus movement and rider effects only for a selected game; autonomous mob behavior remains excluded. |
| Elytra gliding | Conditional | Large | Add for a race or elytra-enabled loadout; exact motion and collision behavior are a meaningful addition to movement. |
| Propelled movement | Conditional | Large | Add individual boosts when their items/enchantments are enabled; shared velocity code does not establish all boost conditions and interactions. |
| Ender pearls and chorus fruit | Include | Medium | Broad utility after projectiles and teleportation exist; preserve destination, damage, and required world-lifetime behavior. |
| Sitting on cushions | Include | Small | A useful lobby detail once passengers, entity support checks, and interaction exist; no autonomous mob AI is needed. |

Cheap relationship features should not be used to understate the much larger vehicle implementations that depend on them. The [26.3 notes](https://feedback.minecraft.net/hc/en-us/articles/48913133328013-Minecraft-Java-Edition-26-3) describe cushions as placeable seating with support rules.

## 13. Presentation and communication

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Player chat | Include | Medium | A basic multiplayer capability; needs session state, audience routing, and connection cleanup. |
| Secure chat | Include | Large | Faithful player-chat handling includes signatures, sessions, and acknowledgement chains; system text alone does not replace it. |
| System messages | Include | Small | Short shared output capability for instructions, errors, and game results. |
| Direct and team messages | Include | Small | Useful routing extensions after player chat, teams, and native commands exist. |
| Rich text | Include | Medium | Reusable components improve nearly all UI; formatting and supported actions need structured encoding rather than ad hoc strings. |
| Player-list entries | Include | Medium | Required for a coherent participant view; extend profile support with audiences, lifecycle, and mode updates. |
| Scoreboards | Include | Medium | Reusable scores and display synchronization support most game types; internal game logic need not execute scoreboard commands. |
| Teams | Include | Medium | Common membership/presentation rules for team games and spectators; source gameplay membership from native match state. |
| Boss bars | Include | Small | Broadly useful progress/timer UI once player audiences exist; localized state and packet handling. |
| Titles and action bar | Include | Small | High reuse for countdowns and round feedback at little code cost. |
| Player HUD state | Include | Small | Needed for combat and round transitions; underlying health, food, and effects are separate dependencies. |
| Locator bar and waypoints | Include | Medium | Useful for team and navigation games; include exact visibility/range rules when enabled and let games disable it. |
| Sounds | Include | Small | Small reusable effect interface once audiences and world positions exist. |
| Particles | Include | Small | Useful for objectives and feedback; support structured parameters and entity/player targeting. |
| World and entity events | Include | Medium | Common visual feedback for supported gameplay; event encoding is separate from causing its game-state change. |
| Maps and markers | Conditional | Large | Add when a game needs functional maps; map pixels, tracked decorations, and item interactions exceed sending a static image. |
| Camera targets | Include | Small | Useful for spectating and scripted observation after entity identity and visibility are established. |
| Dialogs | Include | Medium | A compact general UI opportunity compared with recreating workstation gameplay; support the chosen vanilla dialog/action forms faithfully. |
| Pause-menu content | Include | Small | Useful help and server links after dialogs and rich text exist; no need for a separate menu framework. |
| Server-controlled post effects | Include | Small | A small presentation control that could support many themes; assets remain on the client and gameplay must not depend on successful rendering. |

The [dialog notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-6) describe limited modal UI, not arbitrary screens. The [26.3 post-effect notes](https://feedback.minecraft.net/hc/en-us/articles/48913133328013-Minecraft-Java-Edition-26-3) place shader assets on the client. Both illustrate potentially useful features whose server control surface is much smaller than a new simulation system.

## 14. Progression, statistics, and rewards

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Experience | Include | Medium | Useful vanilla combat/resource state and HUD support; include collection, levels, costs, and death behavior where exposed. |
| Advancement criteria | Replace | Medium / Large | Use typed native game events and achievement conditions; a complete vanilla criterion catalog couples to many omitted activities. |
| Advancement presentation | Include | Medium | Reusable definition/progress packets can display game achievements without implementing every vanilla advancement trigger. |
| Advancement rewards | Replace | Small / Medium | Dispatch explicit native achievement rewards; do not introduce function/data-pack execution solely for vanilla reward definitions. |
| Statistics | Replace | Medium / Large | Track match results and useful player counters; recreating every vanilla statistic has little value for unimplemented activities. |
| Recipe discovery | Conditional | Medium | Add when crafting/recipe-book gameplay requires it; not needed merely to issue items in a loadout. |
| Loot generation | Replace | Medium / Large | Use typed Rust reward tables for game rewards. Faithful drops from supported vanilla blocks/entities remain separate required mechanics. |
| Random sequences | Include | Small | Named deterministic random state is useful for repeatable rounds and tests; preserve vanilla sequence semantics where vanilla content uses them. |

Replacing game rewards does not authorize approximate vanilla drops. If an enabled block, mob, or enchantment exposes context-sensitive loot, its supported behavior and necessary evaluators must be implemented faithfully or the interaction must remain unavailable.

## 15. Commands and customization

Commands are for direct player and staff actions. Illustrative shapes are `/help`, `/join <game>`, `/leave`, `/msg <player> <message>`, `/tp <player>`, and `/kick <player> [reason]`; this is not a finalized command catalog. Prefer one action with a short argument list and readable errors. Complex game logic belongs in Rust, even if it could be represented by a single enormous chat command.

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Command parsing and execution | Replace | Medium / Very large | Implement a finite set of short chat-friendly actions with simple typed arguments and readable errors; no nesting, chaining, expressions, or embedded structured-data editing. |
| Command suggestions | Include | Medium | Useful native command discovery and completion; requires protocol command-tree support, not a complete vanilla executor. |
| Command permissions | Replace | Small / Small | Use a small set of native staff roles and explicit command checks; mirror client permission state where relevant without introducing a general permissions framework. |
| Selectors and execution contexts | Omit | Large | No selector-expression or execute-context language; use named player targets in basic commands and typed Rust queries in game code. |
| World and entity manipulation | Replace | Medium / Large | Keep typed Rust game APIs and short deliberate staff actions such as teleporting; omit a universal manipulation language or inline entity/block definitions. |
| Data inspection and modification | Replace | Medium / Large | Expose targeted readable diagnostics; no arbitrary NBT paths, JSON/component payloads, or general data-editing commands. |
| Command storage | Omit | Medium | Rust game state already has typed storage; a second command-oriented NBT database offers little benefit. |
| Scoreboard logic and triggers | Replace | Small / Medium | Route native commands/actions to game handlers; keep scoreboard display, but omit command-driven arithmetic and triggers that duplicate Rust game logic. |
| Functions | Omit | Large | Command scripts, macros, and function execution are explicitly excluded; native Rust handlers own game logic. |
| Function scheduling | Replace | Small / Medium | Use typed scheduled events internally in Rust; no command scheduling, script names, or execution-context language. |
| Command blocks | Omit | Large | Embedded command programming is explicitly excluded, including in imported maps. |
| Data-pack management | Omit | Very large | Data-pack loading, ordering, enabling, reloading, registry overrides, and execution are explicitly excluded. |
| Data-driven registries | Replace | Medium / Large | Keep required versioned definitions and client synchronization; no user-supplied data-pack loader or registry override/reload system. |
| Tags | Include | Medium | Existing synchronized tags are useful; provide exact memberships for supported behavior and local lookup without requiring arbitrary pack reloads. |
| Predicates and item modifiers | Replace | Medium / Large | Implement conditions and item transformations required by supported mechanics directly in Rust; omit the general data-pack predicate/modifier language. |
| Game-rule commands | Replace | Small / Small | Expose typed native match/environment settings through native staff tools; avoid commands that appear to enable absent gameplay systems. |
| Feature flags and experiments | Replace | Small / Small | Advertise the supported stable feature set. Flag storage is small; implementing experimental mechanics is separate and currently unjustified. |
| Structure placement tools | Replace | Medium / Large | Use map templates, named arena regions, and reset tools; omit vanilla procedural feature and jigsaw command surfaces. |
| Game tests | Replace | Medium / Large | Use small Rust unit tests and selected mechanic reference cases; do not reimplement vanilla test blocks, commands, and a test-server runtime. |
| Data reports | Replace | Small / Medium | Emit targeted reports needed for imported content and supported registries; omit unrelated vanilla report formats. |

Rust implementation is not an exemption from gameplay parity: a native function can implement an enabled block's exact rules without accepting the vanilla command language that can manipulate that block. Protocol-required definitions and codecs remain necessary, but no game requirement should trigger data-pack or command-programming support.

## 16. Persistence and world data

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| World metadata | Replace | Medium / Medium | Keep imported map metadata and native arena configuration; a second writable vanilla metadata model adds compatibility work without a current use case. |
| Chunk storage | Replace | Large / Large | Retain the importer and keep match edits in disposable arena state; general vanilla region writing and persistent survival changes are not required. |
| Entity storage | Replace | Medium / Large | Store map-authored supported entities and explicit native persistent records; discard temporary match actors at teardown. |
| Points of interest | Omit | Large | A villager POI store is unnecessary without villager society; native bot objectives and navigation anchors are different data. |
| Player state | Replace | Medium / Medium | Persist account preferences and deliberate durable progression; keep health, loadouts, position, and respawn state in the appropriate match lifecycle. |
| Player progression | Replace | Medium / Medium | Store native achievements/results and recipe knowledge only where used; a duplicate vanilla-format store adds little value to native game records. |
| Shared saved data | Replace | Medium / Large | Persist explicitly selected native server records; do not clone stores for absent raids, forced chunks, functions, and other survival systems. |
| Clocks and schedules | Replace | Small / Medium | Restart disposable rounds rather than serialize every pending callback; persist clocks/schedules only when a durable native feature needs them. |
| Autosaving | Replace | Medium / Medium | Periodically save dirty durable records; mutable match terrain is intentionally disposable rather than a survival save. |
| Explicit saving | Replace | Small / Medium | Flush Halcyon's durable state for staff actions and shutdown; avoid pretending to save nonexistent vanilla world stores. |
| Data-version upgrades | Replace | Medium / Very large | Version native records and validate supported map imports; offline map conversion avoids a universal vanilla save-upgrade engine. |
| Session locking and load failures | Include | Small | Clear load errors and exclusive access to writable durable stores are useful, bounded infrastructure; immutable assets can be shared safely. |

The input format and durable runtime state need not be the same. Importing vanilla terrain does not obligate Halcyon to write every vanilla save format or recover interrupted rounds as survival worlds.

## 17. Administration and operations

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Startup configuration | Replace | Medium / Medium | Provide native settings for listening, maps, resources, and supported services; omit meaningless survival properties. |
| Initial server setup | Replace | Small / Small | Provide clear configuration validation and startup errors; copying vanilla's setup files is not a gameplay compatibility feature. |
| Player capacity | Include | Small | Explicit admission limits and truthful counts are useful; bot participation must not require one external connection per bot. |
| Operators | Replace | Small / Small | Maintain native staff roles and permissions rather than a second operator database with redundant authority. |
| Allowlist | Include | Small | Useful for testing/private sessions; authenticated UUID membership and admission checks have a small code surface. |
| Player and IP bans | Include | Medium | Useful basic administration with persistence and expiry; reuse identity and admission infrastructure. |
| Kicking | Include | Small | A useful native command on top of disconnect and shared player lookup. |
| Spawn protection | Replace | Small / Small | Use explicit arena/lobby edit permissions and regions; a second radius-based rule around world spawn is redundant. |
| Server code of conduct | Include | Small | A useful localized joining screen with configured text; reuse locale and configuration handling. |
| Console | Include | Medium | Expose the same bounded native administrative actions as chat; do not introduce a console scripting language. |
| RCON | Conditional | Medium | Add if chosen tooling needs it, using the same simple command set; remote access must not add a more complex language. |
| Query service | Conditional | Small | Add if a server directory actually consumes the separate query protocol; ordinary server-list status already covers clients. |
| Server management API | Conditional | Large | Add vanilla management-protocol compatibility only when an actual management client requires it; a generic API is not automatically free tooling value. |
| Management notifications | Conditional | Medium | Add events required by a selected management integration, after the API and native lifecycle events exist. |
| Connection and packet limits | Include | Medium | Extend existing bounds and deadlines with native admission/traffic policies; these support a reliable shared server. |
| Watchdog | Include | Medium | Detect stalled authoritative work and provide usable diagnostics; this requires a view of scheduling beyond one connection. |
| Tick controls | Conditional | Medium | Add when debugging or operations need consistent pause/step/rate controls across arenas; an inconsistent partial freeze is not faithful control. |
| Profiling and metrics | Replace | Medium / Large | Instrument native scheduling, gameplay, and storage with Rust/Linux tools; JVM-specific management facilities do not apply. |
| Empty-server pause | Replace | Small / Small | Suspend arenas only when they have no required human or bot activity; zero human connections alone must not stop a needed bot game. |
| Logging and crash reports | Replace | Medium / Large | Keep concise native lifecycle/error diagnostics and relevant crash context; copying JVM/vanilla diagnostic formats has little value. |
| Graceful shutdown | Include | Medium | Stop admission, end/cancel rounds, save durable state, disconnect clients, and release resources in a defined lifecycle. |

The [official server management API description](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-9) includes authentication, settings, state changes, and notifications. Its value depends on a real tooling integration; it should not be rated Small merely because it exposes existing state through JSON.

## Halcyon-specific capabilities

The following sections are additions, not vanilla rows or assertions about current implementation. They are the work needed to turn shared Minecraft mechanics into the intended minigame platform.

## 18. Match lifecycle, matchmaking, and game rules

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Game definitions and native hooks | Include | Large | Define supported mechanics, participants, objectives, and direct interaction/region-event callbacks; scripted buttons or doors do not require redstone. |
| Match state machine | Include | Medium | Own waiting, preparation, active play, results, cancellation, and cleanup so games cannot leave half-active state behind. |
| Player queues and match assignment | Include | Medium | Place humans into compatible games and teams; start with explicit modes/capacities rather than a speculative matchmaking framework. |
| Bot-filled starts and replacement policy | Include | Medium | Fill empty places, handle departures, and define when humans may replace bots; depends on actual bot and match implementations. |
| Native team membership and objectives | Include | Medium | Own authoritative teams, scoring, elimination, and win conditions; expose this state through UI and combat permissions. |
| Loadouts and spawn assignments | Include | Medium | Apply game-defined equipment and team spawns consistently to humans and bots at entry and respawn. |
| Round timers and scheduled events | Include | Small | Reuse the shared tick scheduler for countdowns, phase changes, and delayed objectives. |
| Disconnect and rejoin policy | Include | Medium | Define slot retention, replacement, spectator admission, and cancellation behavior at match boundaries. |
| Party queues and skill-based matchmaking | Conditional | Large | Add when actual social/ranked modes require them; neither is necessary to prove ordinary bot-filled games work. |

## 19. Arenas, templates, and content boundaries

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Arena templates and map metadata | Include | Medium | Import terrain plus named spawns, regions, and objectives; validate the data each game actually consumes. |
| Independent mutable match worlds | Include | Large | Share immutable template data while giving matches independent edits, entities, inventories, and simulation state. |
| Reset and teardown | Include | Large | Restore the template and discard match-owned work, entities, and references; late events must not mutate a later round. |
| Arena audience isolation | Include | Medium | Prevent terrain/entity/UI updates crossing match boundaries; build on the shared tracking and membership foundations. |
| Supported-content validation | Include | Large | Validate prepared terrain and enabled content; reject dependencies on redstone, runtime world generation, vanilla mob AI, data packs, or command programming. Inert decoration does not enable their behavior. |
| Map preparation and navigation anchors | Include | Medium | Allow authored metadata for objectives and routes without requiring villagers, vanilla POIs, or command blocks. |
| Live creative arena editor | Conditional | Very large | Add only if offline map creation becomes a real workflow limitation; editing, saving, validation, and permissions create a second product surface. |

An imported decoration may remain noninteractive. If players can alter its surroundings in a way that exposes missing supported physics, decay, or fluid behavior, the map restriction or implementation must address that dependency. Dependencies on redstone, runtime world generation, or vanilla mob AI must instead be removed or replaced with prepared content or explicit Rust game logic; they do not trigger reimplementation. Hiding a missing mechanic behind a Small input handler is not a valid size assessment.

## 20. Native bots and humanlike play

These are Halcyon's player bots, not vanilla mob AI. Their inclusion does not require implementing or porting vanilla mob brains, goals, senses, or navigators.

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Native bot participants | Include | Large | Represent bots in shared match/gameplay state without requiring sockets, encryption, or online login for each bot. |
| Bot identity and visual representation | Include | Medium | Give bots stable actor identity and appropriate profiles, equipment, poses, and observer state; reuse entity synchronization. |
| Shared action/rule path | Include | Large | Feed bot actions into the same authoritative movement, inventory, interaction, and combat rules used to validate human actions. |
| Perception and short-term memory | Include | Large | Build decisions from permitted visible/audible events and remembered observations rather than unrestricted world-state knowledge. |
| Humanlike decisions and reaction timing | Include | Very large | Central project requirement: objectives, uncertainty, reaction delays, aim, movement choices, and behavior variation require substantial authored behavior. |
| Bot combat and item use | Include | Large | Choose targets, aim, time attacks, use equipment, and retreat through shared mechanics; requires perception, decisions, navigation, and combat. |
| Bot navigation and local steering | Include | Large | Route through supported player movement and arena geometry; avoid bots that merely teleport between authored points. |
| Navigation invalidation after edits | Include | Large | Building games change routes; update affected navigation and recover from blocked or invalid plans. |
| Team coordination and game objectives | Include | Large | Bots must contribute to the selected game's goals and roles, not just wander or attack the nearest entity. |
| Difficulty and behavior profiles | Include | Medium | Configure decision accuracy, reaction timing, and choices; do not secretly change supported physics or damage for bot convenience. |
| Reproducible bot scenarios | Include | Medium | Reproduce individual navigation/decision failures with controlled inputs and seeds; keep this focused rather than building a universal replay platform. |
| Learned behavior or training pipeline | Conditional | Very large | Add only when explicit quality goals cannot be met with authored behavior and justify the tooling; humanlike behavior does not by itself require machine learning. |

These are deliberately substantial Includes: they directly deliver Halcyon's stated purpose. Cost discipline should remove unrelated vanilla ecosystems, not disguise the size of the project's differentiating feature. Route planning and decision quality can evolve separately from exact supported gameplay mechanics.

## 21. Player records and game presentation

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Durable player identity and preferences | Include | Medium | Associate human account UUIDs with settings and selected durable records; keep temporary match state out of the account model. |
| Match results and progression records | Include | Medium | Record outcomes and deliberate native progression without cloning survival saves or every vanilla statistic. |
| Game selection and lobby flow | Include | Medium | Provide commands, dialogs, or supported inventory menus for joining, leaving, and understanding games. |
| Spectator and results flow | Include | Medium | Connect elimination, spectating, result UI, and return-to-lobby behavior using shared player/presentation primitives. |
| Rankings, economy, and unlock systems | Conditional | Large | Add specific systems only when game design requires them; a broad platform does not automatically need a shop economy or ranked ladder. |

## 22. Scheduling and operational foundations

| Feature | Recommendation | Code size | Use case / rationale |
| --- | --- | --- | --- |
| Authoritative simulation ownership | Include | Large | Define ownership of match/entity/world state and action ordering; connection tasks alone are not a simulation architecture. |
| Multi-core execution | Include | Large | Allow independent work and arenas to use the machine's cores; do not treat the current fixed-core executor as a permanent restriction. |
| Separate decision and simulation scheduling | Include | Medium | Schedule bot thinking independently from required physics ticks; reducing decision frequency must not approximate enabled vanilla mechanics. |
| Bounded queues and admission | Include | Medium | Bound pending actions, connection work, and jobs and define rejection/backpressure; shares existing lifecycle and transport safeguards. |
| Reusable data and work buffers | Include | Medium | Extend existing shared terrain/configuration encodings and reusable storage where ownership permits; include invalidation for changed data. |
| Off-executor durable I/O | Include | Medium | Keep account/result writes and other blocking operations out of gameplay execution; existing authentication offloading is a precedent, not the complete subsystem. |
| Focused diagnostics and measurement | Include | Medium | Expose enough state to understand failures and resource use without adding per-packet output or an all-purpose observability platform. |

These architectural constraints are separate from the code-size ratings. No population capacity or performance improvement is claimed here. Implementations must be measured against actual workloads; this document does not select a threading algorithm, commit to a bot count, or require a distributed server architecture.
