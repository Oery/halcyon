# Vanilla Minecraft Java server feature inventory

This inventory describes capabilities of the vanilla Minecraft Java Edition dedicated server, using **26.3** as the baseline to match Halcyon's registry data. It is a reference for later feature selection, not a requirement for vanilla parity or an assessment of what Halcyon already implements. Inclusion here makes no recommendation about Halcyon's design, priorities, or performance costs.

Each row identifies a capability that can be discussed separately. Related capabilities still depend on one another: sending a projectile to clients, simulating its motion, and applying its damage are separate responsibilities. Examples identify behavior families rather than enumerate every block, item, mob, recipe, or command. Configurable and command-only capabilities are included even when disabled or unused in a default world.

The scope includes gameplay, client synchronization, persistence, customization, and dedicated-server operations. Purely client-side rendering and settings, Bedrock features, Realms hosting, mods, plugins, proxy networks, and Halcyon's own minigames and humanlike bots are outside this inventory. Protocol facilities intended for custom servers do not by themselves imply built-in vanilla gameplay or routing features.

References near each section point to relevant official guides and release notes; older notes explain features inherited by the baseline, not their old defaults or wire formats. The [official 26.3 release notes](https://feedback.minecraft.net/hc/en-us/articles/48913133328013-Minecraft-Java-Edition-26-3) establish the version baseline. For later packet-level work, use the repository's designated [Java Edition protocol packet reference](https://minecraft.wiki/w/Java_Edition_protocol/Packets), checking the exact target version and packet section. This document deliberately does not specify packet IDs or field layouts.

## 1. Connections and compatibility

| Feature | Vanilla behavior |
| --- | --- |
| Server-list status | Answers status requests with the description, version, player counts, optional sample profiles, and server icon. |
| Status latency measurement | Responds to server-list ping requests so the client can measure round-trip latency. |
| Protocol negotiation | Processes the handshake and requested connection intent; rejects incompatible clients as appropriate. |
| Online authentication | Verifies joining players through Minecraft session services and establishes their authenticated profiles. |
| Offline-mode identity | Can admit players without online session verification when configured, using offline identities. |
| Profile properties | Uses player UUIDs, names, and profile properties such as skin textures when presenting players to clients. |
| Encrypted transport | Negotiates encryption during online login and encrypts subsequent connection traffic. |
| Packet compression | Negotiates a compression threshold and handles compressed packet framing. |
| Configuration phase | Exchanges client settings, enabled features, registry information, tags, and other configuration before entering Play. |
| Reconfiguration | Can return a connected client to configuration when required, such as by changes to synchronized data. |
| Client preferences | Receives preferences such as language, requested view distance, chat settings, main hand, and visible skin layers. |
| Resource-pack delivery | Requests the configured server resource pack, receives loading results, and can require acceptance. |
| Keepalives and inactivity | Checks connection responsiveness and can disconnect idle players according to server settings. |
| Disconnect handling | Ends connections with a reason and removes departed players from the world and other clients' views. |
| Incoming server transfers | Can accept clients redirected by another server when incoming transfers are enabled; this is not a built-in proxy or matchmaking system. |

Reference: the [1.20.5 networking changes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-20-5) describe transfer acceptance, reconfiguration-related behavior, and protocol facilities for custom servers. The packet reference linked above is the follow-up reference for individual connection exchanges.

## 2. Players, movement, and lifecycle

| Feature | Vanilla behavior |
| --- | --- |
| Survival mode | Applies resource consumption, ordinary mining, health, hunger, and survival interaction rules. |
| Creative mode | Allows creative inventory actions, rapid block removal, flight, and broad damage immunity. |
| Adventure mode | Restricts block placement and destruction according to item permissions while retaining other player interactions. |
| Spectator mode | Allows observation, movement through terrain, and spectating other entities with restricted world interaction. |
| Hardcore rules | Enforces the world's hardcore restrictions and prevents ordinary survival respawning after death. |
| Ground movement | Accepts and validates player position and rotation updates and synchronizes accepted movement to observers. |
| Collision and stepping | Checks movement against block collision shapes, entity interactions, step height, and world limits. |
| Sprinting and sneaking | Tracks movement states that affect speed, interactions, poses, and visibility rules. |
| Jumping and falling | Handles jump-related state, airborne movement checks, landing, and accumulated fall distance. |
| Swimming and crawling | Supports swimming and low-profile poses with their corresponding movement and collision behavior. |
| Climbing | Handles traversal of ladders, vines, scaffolding, and other climbable blocks. |
| Movement surfaces and media | Applies effects from water currents, bubbles, ice, slime, honey, cobwebs, powder snow, and similar surroundings. |
| Flight permissions | Tracks whether a player may fly and whether flight is active; checks unauthorized flying. |
| Movement abilities and attributes | Applies player attributes and abilities that affect speed, reach, scale, gravity, and related behavior. |
| Teleportation | Changes player position or rotation and coordinates client confirmation and movement synchronization. |
| Initial spawning | Selects a world entry position using world spawn and applicable spawn-location rules. |
| Personal respawn points | Stores and validates bed, respawn-anchor, or command-assigned respawn locations. |
| Death processing | Records death, reports its cause, handles drops and experience, and updates applicable statistics and scores. |
| Respawning | Recreates the player's living state and synchronizes position, dimension, abilities, and other reset state. |
| Dimension transitions | Moves players between dimensions and updates dimension-dependent client state. |
| Player poses and actions | Tracks states such as sleeping, item use, arm swings, and selected equipment for other clients. |
| End completion flow | Handles the End exit and the associated completion/credits transition for players. |

References: [game modes](https://help.minecraft.net/hc/en-us/articles/360058743992-Minecraft-Differences-Between-Creative-Survival) and [spawning and dying](https://www.minecraft.net/en-us/article/spawning-and-dying). In the 26.3 baseline, spectators can also use portals, as described in the baseline release notes.

## 3. Combat and survival

| Feature | Vanilla behavior |
| --- | --- |
| Melee targeting | Checks whether an attack can reach and affect its target, including applicable distance and visibility restrictions. |
| Attack cooldowns | Scales ordinary melee attacks according to attack recovery and attack-speed attributes. |
| Critical hits | Applies additional melee damage when the attacker meets critical-hit conditions. |
| Sweeping attacks | Applies eligible sword attacks to nearby secondary targets under sweeping rules. |
| Mace smashes | Applies fall-dependent attacks and their associated damage and movement effects. |
| Spear attacks | Handles spear jabs and charges, including their reach, timing, velocity-dependent effects, and dismounting rules. |
| Knockback | Applies attack impulses and knockback resistance to affected entities. |
| Shields and blocking | Handles blocking direction, damage reduction, durability costs, and shield disabling where applicable. |
| Ranged weapon use | Handles bow charging, crossbow loading, ammunition selection, and weapon release. |
| Projectile motion and hits | Simulates arrows, tridents, thrown items, and other projectiles, including collisions and impact effects. |
| Projectile ownership | Attributes hits to shooters and handles applicable reflection, piercing, recovery, and pickup rules. |
| Damage types and attribution | Distinguishes damage sources, attacker responsibility, immunities, and cause-specific behavior. |
| Damage cooldowns | Applies invulnerability windows and rules for repeated damage. |
| Armor and protection | Reduces damage using armor, toughness, enchantments, resistance, and damage-specific exceptions. |
| Health and absorption | Tracks health and temporary absorption separately and synchronizes changes to the player. |
| Healing and regeneration | Restores health through food-related regeneration, effects, and other healing sources. |
| Hunger, saturation, and exhaustion | Consumes food resources through actions and applies starvation and regeneration rules. |
| Eating and drinking | Processes use duration, consumable effects, item consumption, and replacement containers. |
| Status effects | Applies timed and instant effects, amplifiers, removal, and client-visible effect state. |
| Splash and lingering effects | Applies area potion effects and persistent effect clouds to eligible entities. |
| Environmental damage | Handles falls, drowning, fire, lava, freezing, suffocation, the void, and other environmental damage sources. |
| Air and exposure state | Tracks remaining air, burning duration, and freezing buildup that can lead to environmental damage. |
| Death prevention | Applies effects such as a totem rescuing an otherwise dying player. |
| Enchantment effects | Executes applicable combat, movement, equipment, and survival enchantment effects. |
| Player-versus-player rules | Controls player damage and team-related friendly-fire behavior. |

References: the [1.21 release notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21) cover mace combat and data-driven enchantments; the [1.21.11 release notes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-11) cover spear attacks. Their numeric tuning is outside this inventory.

## 4. Items and inventory

| Feature | Vanilla behavior |
| --- | --- |
| Item stacks | Tracks item type, count, maximum stack size, and stack compatibility. |
| Item components | Stores per-stack properties such as custom names, lore, enchantments, contents, and custom data. |
| Player inventory | Maintains storage slots, hotbar, selected slot, armor, offhand, and the carried cursor stack. |
| Equipment changes | Equips and removes items, including applicable slot restrictions and attribute changes. |
| Inventory clicks | Processes pickup, swap, split, quick-move, drag distribution, collection, and slot-specific click rules. |
| Inventory reconciliation | Synchronizes slots and carried items and corrects client predictions when they disagree with server state. |
| Container sessions | Opens and closes container screens, assigns their slots, and checks continued access. |
| Container properties | Synchronizes screen-specific values such as cooking progress, fuel, enchanting offers, or repair costs. |
| Item pickup | Transfers eligible dropped stacks into inventories and synchronizes collection. |
| Item dropping | Creates dropped item entities from manual drops, deaths, block drops, and other sources. |
| Dropped-item lifecycle | Handles pickup delays, stack merging, damage, and eventual despawning. |
| Durability and breakage | Consumes durability, applies durability-related effects, and removes broken equipment. |
| Item-use cooldowns | Prevents immediate repeated use of items with cooldowns and sends cooldown state to clients. |
| Nested item storage | Maintains portable contents in items such as bundles and shulker boxes. |
| Ender chest storage | Provides player-specific storage shared across that player's ender chest accesses. |
| Books and written content | Processes book editing, signing, stored pages, and lectern reading state. |
| Fishing | Simulates casting, bite timing, loot retrieval, and hooking or pulling entities. |
| Navigation item targets | Maintains world spawn, lodestone positions, and last-death locations used by navigation items. |
| Creative item acquisition | Handles creative slot changes and item picking under creative permissions. |

References: [crafting and inventory basics](https://www.minecraft.net/en-us/article/how-craft) and the [item-component introduction](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-20-5). Component names and schemas must be checked against 26.3 before implementation.

## 5. Crafting, processing, and trading

| Feature | Vanilla behavior |
| --- | --- |
| Grid crafting | Matches shaped and shapeless recipes in the player grid and crafting table. |
| Crafting outputs and remainders | Consumes ingredients, produces output stacks, and returns or replaces ingredient containers as required. |
| Recipe book interaction | Tracks recipe availability and handles requests to place recipe ingredients into supported crafting screens. |
| Furnace processing | Handles smelting, fuel consumption, cooking progress, output collection, and stored experience. |
| Specialized cooking | Supports blast furnaces, smokers, and campfire cooking with their applicable recipes and timing. |
| Brewing | Processes brewing ingredients, fuel, potion transformations, and bottle slots. |
| Enchanting table | Generates offers using the item, enchantment rules, surrounding bookshelves, lapis, and player experience. |
| Anvil | Combines and repairs equipment, applies enchanted books, renames items, and computes costs. |
| Grindstone | Repairs compatible equipment, removes applicable enchantments, and returns experience. |
| Smithing | Applies equipment upgrades and armor trims using templates and required inputs. |
| Stonecutting | Offers block conversion recipes with their own ingredient and output handling. |
| Loom | Applies banner patterns using dyes and applicable pattern items. |
| Cartography table | Handles map copying, expansion, and locking. |
| Merchant transactions | Presents offers and validates inputs, output, stock, and trade completion. |
| Trade economics | Updates applicable prices, demand, reputation discounts, merchant experience, levels, and restocking. |
| Bartering | Exchanges a piglin's accepted payment for randomly selected barter loot without a trading screen. |

References: [crafting](https://www.minecraft.net/en-us/article/how-craft), [villager trading behavior](https://www.minecraft.net/en-us/article/village---pillage-out-java-), and the [26.1 data-driven trade definitions](https://feedback.minecraft.net/hc/en-us/articles/44551668333837-Minecraft-Java-Edition-26-1). Potion recipes are also data-driven in 26.3, as documented in the baseline release notes.

## 6. Blocks and interaction

| Feature | Vanilla behavior |
| --- | --- |
| Block states | Represents each block's state properties, such as orientation, connections, age, or waterlogging. |
| Block placement | Checks placement position, supporting conditions, permissions, replaceability, and resulting state. |
| Block breaking | Handles start, cancel, and completion, with mining speed, tool suitability, and game-mode restrictions. |
| Break feedback | Broadcasts block-damage progress and resulting block changes to nearby players. |
| Block drops | Produces block loot according to tools, enchantments, block state, and applicable rules. |
| Interaction validation | Checks reach, target, held item, permissions, and whether a block or item action takes precedence. |
| Predicted-action correction | Acknowledges relevant player actions and corrects predicted blocks or items when an action is rejected. |
| Neighbor and shape updates | Recomputes connected states, support requirements, and reactions when adjacent blocks change. |
| Block entities | Maintains additional block-associated state, such as inventories, text, timers, or spawner settings. |
| Doors, trapdoors, and gates | Handles opening, closing, paired parts, and manual or powered access rules. |
| Beds and respawn anchors | Checks use conditions, tracks occupied or charged state, and applies their respawn behavior. |
| Straw beds | Allows one-use sleeping without assigning a respawn point. |
| Signs | Stores text on both sides, processes authorized edits, and supports dye, glow, wax, and applicable text actions. |
| Shelves and storage interactions | Supports direct item storage or swaps and container-specific interactions beyond ordinary inventory screens. |
| Jukeboxes | Inserts and ejects music discs, tracks playback, and produces applicable sound and redstone output. |
| Buckets and fluid containers | Collects and places fluids or bucketable entities and handles applicable cauldron interactions. |
| Tool transformations | Applies actions such as stripping, tilling, path creation, scraping, waxing, and shearing. |
| Farming and harvesting | Handles crop planting and harvesting, fertilization, and block-specific produce. |
| Archaeology | Processes brushing progress, suspicious-block contents, and excavation results. |
| Beacons and conduits | Checks supporting structures or surroundings and applies effects to eligible nearby entities. |
| Trial spawners and vaults | Runs encounter and reward states, including player eligibility and applicable ominous variants. |
| Structure and jigsaw blocks | Saves or places structures and assembles configured structure pieces for world building. |

References: [1.21 trial mechanics](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21), [1.21.9 shelves](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-9), and the baseline release notes for straw beds. Exact interactions remain specific to each block family.

## 7. World simulation

| Feature | Vanilla behavior |
| --- | --- |
| Game ticking | Advances simulation on game ticks, normally targeting 20 ticks per second. |
| Scheduled block ticks | Executes delayed block behavior, such as repeater transitions, button releases, and observer pulses. |
| Scheduled fluid ticks | Advances pending water and lava updates according to their timing rules. |
| Random ticks | Samples eligible blocks for processes such as plant growth, decay, and environmental changes. |
| Block-entity ticking | Advances active block entities such as furnaces and brewing stands. |
| Entity ticking | Advances active entities, their timers, interactions, and type-specific behavior. |
| Fluid simulation | Handles flowing fluids, source formation, waterlogging interactions, and fluid mixing. |
| Falling blocks | Converts unsupported falling blocks into moving entities and resolves their landing or destruction. |
| Fire | Handles ignition, burning duration, spread, extinguishing, and consumption of flammable blocks. |
| Explosions | Computes affected entities and blocks, exposure, impulses, destruction, and drops. |
| Lighting | Computes and propagates block light and sky light after relevant world changes. |
| Plant growth and spread | Advances crops, saplings, vines, fungi, and other vegetation under their own conditions. |
| Decay and material changes | Handles processes such as leaf decay, copper oxidation, ice melting, and coral death. |
| Vibration events | Emits and propagates game events used by sculk sensors, shriekers, wardens, and related behavior. |
| Area effects | Applies beacon, conduit, cloud, and similar effects according to position and eligibility. |

Reference: [tick controls and timing](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-20-3). The tick mechanisms here describe behavior categories, not a required implementation schedule for Halcyon.

## 8. Redstone and automation

| Feature | Vanilla behavior |
| --- | --- |
| Power sources | Produces redstone power from switches, buttons, plates, sensors, and other inputs. |
| Signal propagation | Updates signal strength through dust and powered blocks according to Java redstone rules. |
| Repeaters | Applies signal delay, directionality, and locking. |
| Comparators | Compares or subtracts signals and reads supported block contents or state. |
| Observers | Emits pulses in response to detected block changes. |
| Pistons | Pushes or pulls eligible blocks and resolves moving blocks and affected entities. |
| Powered mechanisms | Drives doors, lamps, rails, bells, trapdoors, and other powered blocks. |
| Note blocks | Plays notes according to tuning, instrument selection, and player or redstone activation. |
| Dispensers | Executes item-specific automated actions such as firing projectiles, equipping entities, or placing fluids. |
| Droppers | Ejects items or transfers them into an adjacent inventory. |
| Hoppers | Pulls, pushes, and collects items with transfer timing and powered locking. |
| Automated crafting | Uses crafter recipes, enabled slots, and redstone activation to produce and eject items. |
| TNT ignition | Primes TNT from supported triggers and creates the corresponding timed entity. |
| Redstone ordering behavior | Resolves update order, quasi-connectivity, and block-specific timing that Java contraptions can depend on. |

References: [redstone overview](https://www.minecraft.net/en-us/article/block-week-redstone) and the [1.21 crafter](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21). This inventory includes ordinary Java behavior, not optional experimental redstone changes.

## 9. Chunks, generation, and dimensions

| Feature | Vanilla behavior |
| --- | --- |
| Chunk loading and unloading | Brings world regions into memory as needed and releases regions no longer required. |
| Terrain generation | Generates terrain from a seed using the selected world generator and settings. |
| Biome generation | Assigns biomes that affect terrain content, spawning, environmental behavior, and client presentation. |
| Structure generation | Places villages, strongholds, trial chambers, and other structures with their associated data. |
| Features and decoration | Places ores, vegetation, lakes, and other configured world-generation features. |
| World presets | Supports configured generation choices such as normal, superflat, amplified, and single-biome worlds. |
| Chunk streaming | Sends chunk terrain, biomes, block entities, height information, and light data to observing clients. |
| Chunk batch feedback | Coordinates chunk batches with client acknowledgements and reported receiving capacity. |
| View distance | Controls which surrounding chunks are sent to a player, subject to server and client settings. |
| Simulation distance | Limits nearby regions where simulation runs independently of the visible terrain range. |
| Chunk loading tickets | Keeps chunks loaded for reasons such as players, portals, ender pearls, or explicit forced loading. |
| Chunk simulation levels | Distinguishes loaded terrain from regions eligible for block and entity ticking. |
| Chunk and biome updates | Sends incremental changes instead of requiring a complete terrain resend for every modification. |
| Built-in dimensions | Supports the Overworld, Nether, and End and their respective world rules and content. |
| Custom dimensions | Loads data-pack-defined dimensions and dimension types with compatible generation and environment settings. |
| Portals and gateways | Locates or creates destinations, applies coordinate conversion where applicable, and handles portal cooldowns. |
| World borders | Maintains borders per dimension, including size changes, warnings, and damage behavior. |
| World height and limits | Enforces build-height constraints and other dimension or coordinate limits. |

Version note: fixed spawn chunks are absent from this baseline. The [1.21.9 world-loading changes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-9) removed them and describe other loading reasons; the same release made world borders dimension-specific. Chunk streaming remains a separate capability from generating or simulating terrain.

## 10. Environment and world rules

| Feature | Vanilla behavior |
| --- | --- |
| World clocks | Maintains data-driven clocks whose time can advance, pause, resume, or be changed by commands. |
| Day and night | Uses time-dependent state for daylight, celestial progression, and applicable gameplay behavior. |
| Timelines | Applies configured changes to environment attributes over time using an associated world clock. |
| Weather | Tracks clear weather, rain, and thunder and synchronizes changes to clients. |
| Lightning | Selects lightning events and applies their entity and block interactions. |
| Sleeping | Tracks eligible sleeping players, checks sleep conditions, and advances time when the required threshold is met. |
| Difficulty | Applies peaceful, easy, normal, hard, and local-difficulty effects to applicable gameplay. |
| Regional climate behavior | Handles biome-dependent precipitation, freezing, snow accumulation, and related conditions. |
| Environment attributes | Supplies dimension-, biome-, time-, or weather-dependent values controlling supported visual and gameplay behavior. |
| Dimension-specific hazards | Applies environmental rules such as water evaporation and invalid bed or anchor use. |
| Game-rule state | Stores configurable gameplay rules and applies changes to their affected systems. |

References: [1.21.11 environment attributes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-11) and [26.1 world clocks and timelines](https://feedback.minecraft.net/hc/en-us/articles/44551668333837-Minecraft-Java-Edition-26-1). These data-driven capabilities do not mean arbitrary client rendering code executes on the server.

## 11. Entities and mobs

| Feature | Vanilla behavior |
| --- | --- |
| Entity identity and lifecycle | Allocates identities, creates entities, removes them, and distinguishes temporary removal from persistent storage. |
| Entity tracking | Selects which players observe each entity according to distance and applicable tracking rules. |
| Entity synchronization | Sends spawning, movement, rotation, velocity, metadata, equipment, status, and removal updates. |
| Entity physics | Advances non-player motion, gravity, drag, collision, and interactions with blocks or fluids. |
| Entity interaction | Handles using, attacking, mounting, naming, or otherwise interacting with applicable entity types. |
| Living-entity attributes | Maintains base attributes and modifiers affecting health, movement, damage, size, and other properties. |
| Natural spawning | Selects eligible spawn locations and populations using environment, distance, light, biome, and population rules. |
| Explicit spawning | Creates entities through commands, spawn eggs, spawner blocks, structures, or scripted vanilla mechanisms. |
| Despawning and persistence | Removes eligible distant mobs while retaining entities protected by naming or other persistence conditions. |
| Mob decisions | Runs goals, activities, memories, sensors, target selection, and type-specific behavior. |
| Pathfinding and navigation | Finds and follows routes appropriate to walking, swimming, flying, or other mob movement. |
| Perception and hostility | Detects targets, checks visibility, handles anger, and reacts to threats or nearby events. |
| Mob combat | Executes melee, ranged, and special attacks with the mob's targeting and timing rules. |
| Mob world interaction | Allows eligible mobs to pick up items or change blocks, subject to applicable griefing and interaction rules. |
| Breeding and growth | Handles food, breeding eligibility, offspring, age, and growth stages. |
| Taming and ownership | Records owners and applies commands or behavior for eligible tameable mobs. |
| Leashes and home areas | Restricts or guides eligible entities using leashes and applicable home-position behavior. |
| Villager society | Maintains points of interest, beds, jobs, schedules, breeding, reputation, and golem-related behavior. |
| Raids, patrols, and sieges | Coordinates multi-entity events, wave state, targets, and rewards. |
| Boss encounters | Runs dragon, wither, and other encounter-specific phases, attacks, and lifecycle behavior. |
| Mob transformations | Applies transformations such as curing, zombification, lightning conversions, or type-specific growth. |
| Utility mob behavior | Runs specialized activities such as allay collection, copper golem sorting, or bee pollination. |
| Decorative entities | Maintains armor stands, paintings, item frames, and other entities used for decoration. |
| Display and interaction entities | Supports configurable block, item, and text displays and interaction targets for custom content. |
| Mannequins | Supports command-spawned player-shaped living entities with profiles, equipment, poses, attributes, and effects. |

References: [villager behavior](https://www.minecraft.net/en-us/article/village---pillage-out-java-) and [1.21.9 copper golems and mannequins](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-9). Mannequins are not autonomous humanlike player bots; their presence does not imply vanilla provides Halcyon's planned bot behavior.

## 12. Vehicles and transport

| Feature | Vanilla behavior |
| --- | --- |
| Mounting and passengers | Maintains vehicle/passenger relationships and synchronizes attaching and dismounting. |
| Riding input | Receives rider controls and applies vehicle-specific movement and restrictions. |
| Boats and chest boats | Handles steering, water and land movement, collisions, passengers, and applicable storage. |
| Minecarts and rails | Simulates rail following, powered and detector rails, collisions, and cart-specific behavior. |
| Land mounts | Supports controlling eligible horses, camels, pigs, striders, and other rideable mobs. |
| Mount equipment and storage | Handles saddles, harnesses, armor, and inventories for applicable mounts. |
| Flying mounts | Supports happy ghast riding and its multiple passengers. |
| Underwater mounts | Supports nautilus riding and its underwater movement and rider-related effects. |
| Elytra gliding | Tracks gliding state, validates movement, applies equipment wear, and handles collisions. |
| Propelled movement | Applies movement effects such as firework boosts, riptide, wind charges, and spear lunges. |
| Ender pearls and chorus fruit | Performs item-triggered teleportation with destination checks and associated side effects. |
| Sitting on cushions | Allows a player to sit on a placed cushion and handles its support and removal rules. |

References: [1.21.6 happy ghasts](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-6), [1.21.11 nautiluses and spears](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-11), and the baseline release notes for cushions. Portals and dimension travel are listed with chunks and dimensions.

## 13. Presentation and communication

| Feature | Vanilla behavior |
| --- | --- |
| Player chat | Receives and distributes player messages using the applicable chat type and message context. |
| Secure chat | Handles player chat sessions, signatures, message-chain validation, and acknowledgements under the configured policy. |
| System messages | Sends server-generated text and command feedback independently of signed player messages. |
| Direct and team messages | Routes private and team messages through the corresponding vanilla commands. |
| Rich text | Sends styled and translated text, including supported hover, click, score, selector, and object components. |
| Player-list entries | Updates listed players, profiles, latency, game modes, display names, and applicable ordering. |
| Scoreboards | Maintains objectives and scores and displays them in supported client locations. |
| Teams | Controls team identity, name formatting, nameplate visibility, collision rules, and friendly-fire settings. |
| Boss bars | Displays named progress bars with configured values, colors, styles, and player audiences. |
| Titles and action bar | Displays title/subtitle sequences and transient action-bar text. |
| Player HUD state | Synchronizes health, food, experience, air-related entity state, and other server-dependent indicators. |
| Locator bar and waypoints | Controls tracked waypoints, transmission/reception ranges, styles, and visibility for the locator bar. |
| Sounds | Requests positional or entity-associated sounds and stops applicable sounds for selected players. |
| Particles | Requests particle effects with type-specific parameters and visibility settings. |
| World and entity events | Triggers client feedback such as block effects, damage animations, pickup effects, and entity status events. |
| Maps and markers | Updates explored map pixels, decorations, banners, and relevant tracked positions. |
| Camera targets | Selects a supported entity as a player's camera target, notably during spectating. |
| Dialogs | Provides simple modal messages, inputs, and actions through vanilla's data-driven dialog facility. |
| Pause-menu content | Exposes server links and configured dialogs through supported pause-menu and quick-action facilities. |
| Server-controlled post effects | Requests named postprocessing effects; the client's resources determine whether those effects exist and render. |

References: [secure chat](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-19-1), [1.21.6 dialogs and waypoints](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-6), and the baseline release notes for post effects. Secure chat is listed as server behavior; client reporting interfaces and Mojang's external moderation service are outside the inventory.

## 14. Progression, statistics, and rewards

| Feature | Vanilla behavior |
| --- | --- |
| Experience | Awards and collects experience, tracks levels and progress, and applies experience costs or loss. |
| Advancement criteria | Evaluates event-driven criteria, requirements, and completion state for each player. |
| Advancement presentation | Sends advancement definitions and progress and controls announcement or toast-related state. |
| Advancement rewards | Grants configured experience, loot, recipes, or function execution on completion. |
| Statistics | Tracks counters such as distance, deaths, kills, item use, and block interactions and supplies them to clients. |
| Recipe discovery | Unlocks recipes through applicable triggers and persists player knowledge. |
| Loot generation | Evaluates loot tables, pools, conditions, functions, and context for blocks, mobs, containers, fishing, and other rewards. |
| Random sequences | Maintains named random-sequence state used by applicable loot and command behavior. |

Reference: the [1.21 advancement and loot changes](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21) provide examples of these systems. Recipe-screen interaction is listed under crafting; experience-based survival and equipment effects use the shared player experience state.

## 15. Commands and customization

| Feature | Vanilla behavior |
| --- | --- |
| Command parsing and execution | Parses supported commands, validates arguments, executes them in context, and returns feedback. |
| Command suggestions | Sends available command structure and supplies argument completions according to permissions. |
| Command permissions | Checks command-source permission levels and distinguishes player, console, function, and command-block contexts. |
| Selectors and execution contexts | Selects entities and positions and changes execution context through vanilla command facilities. |
| World and entity manipulation | Provides commands for setting blocks, filling or cloning areas, summoning entities, teleporting, and similar operations. |
| Data inspection and modification | Reads and changes supported entity, block-entity, item, and command-storage data under vanilla restrictions. |
| Command storage | Maintains namespaced structured data for functions and commands. |
| Scoreboard logic and triggers | Uses scores for arithmetic, conditions, and player-accessible trigger objectives in command-driven content. |
| Functions | Runs command sequences, including supported macro functions and returned results. |
| Function scheduling | Schedules function execution for future game times and manages scheduled work. |
| Command blocks | Runs impulse, repeating, and chain command blocks with conditional and powered execution. |
| Data-pack management | Loads, orders, enables, disables, and reloads compatible data packs. |
| Data-driven registries | Loads supported definitions such as dimension types, biomes, damage types, enchantments, and dialogs. |
| Tags | Groups supported registry entries for use by behavior, recipes, predicates, and commands. |
| Predicates and item modifiers | Evaluates reusable conditions and transformations for command and loot-driven content. |
| Game-rule commands | Exposes supported game rules for inspection and runtime modification. |
| Feature flags and experiments | Tracks enabled vanilla feature sets and compatible experimental data-pack features. |
| Structure placement tools | Places configured features, structures, templates, or jigsaw assemblies through supported commands. |
| Game tests | Provides data-defined block-based tests, test blocks, test execution commands, and a dedicated test-server mode. |
| Data reports | Generates supported registry and content reports for technical inspection using the vanilla data-generation tooling. |

References: [1.21 data-driven enchantments](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21), [1.21.5 Game Tests](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-5), and [26.3 data-pack changes](https://feedback.minecraft.net/hc/en-us/articles/48913133328013-Minecraft-Java-Edition-26-3). Built-in data-pack facilities are included; registering new Java test functions through modded code is not a vanilla feature requirement.

## 16. Persistence and world data

| Feature | Vanilla behavior |
| --- | --- |
| World metadata | Saves world identity, generation settings, dimension state, spawn information, and other world-level data. |
| Chunk storage | Reads and writes terrain, biomes, block entities, lighting-related data, height information, and scheduled ticks. |
| Entity storage | Persists eligible entities and reconstructs their saved state when their regions are loaded. |
| Points of interest | Persists information used to locate and claim relevant blocks, especially for villagers. |
| Player state | Saves position, dimension, inventory, equipment, abilities, health, food, experience, and respawn information. |
| Player progression | Persists advancement progress, statistics, and known recipes. |
| Shared saved data | Persists maps, scoreboards, raids, boss bars, command storage, forced chunks, and other applicable world data. |
| Clocks and schedules | Persists world-clock state and scheduled function events. |
| Autosaving | Periodically writes dirty world and player state. |
| Explicit saving | Provides save commands and controls for flushing data or suspending automatic saves. |
| Data-version upgrades | Converts supported older save formats and can perform a full world upgrade during startup. |
| Session locking and load failures | Guards world access with a session lock and reports unreadable, incompatible, or damaged saved data. |

Reference: [26.1 world-storage changes](https://feedback.minecraft.net/hc/en-us/articles/44551668333837-Minecraft-Java-Edition-26-1) reorganized player and world data. This inventory describes persisted capabilities rather than prescribing historical filenames. Saving is not a built-in backup rotation or disaster-recovery service.

## 17. Administration and operations

| Feature | Vanilla behavior |
| --- | --- |
| Startup configuration | Reads dedicated-server properties and launch options for networking, worlds, limits, and enabled services. |
| Initial server setup | Creates required initial configuration and requires acceptance of the server EULA before running a world. |
| Player capacity | Applies the configured maximum player count and applicable operator exceptions. |
| Operators | Maintains operator identities and permission levels. |
| Allowlist | Restricts admission to allowed identities when enabled and supports changes through commands or management tools. |
| Player and IP bans | Stores admission bans with supported metadata and permits removal of bans. |
| Kicking | Disconnects selected players with an administrative reason. |
| Spawn protection | Restricts applicable block interactions around world spawn according to server settings and operator status. |
| Server code of conduct | Can present configured server rules for joining players to acknowledge. |
| Console | Accepts administrative commands from the dedicated-server process and emits command output and logs. |
| RCON | Optionally accepts authenticated remote console commands and returns their output. |
| Query service | Optionally exposes server information through the separate query protocol. |
| Server management API | Optionally exposes authenticated JSON-RPC over WebSocket for server state, settings, players, and management operations. |
| Management notifications | Reports supported state changes to management clients, including world-upgrade events in the 26.3 API. |
| Connection and packet limits | Applies configured limits and validation to connection traffic and disconnects clients for applicable violations. |
| Watchdog | Detects excessively long server ticks and can produce diagnostic output and stop an unresponsive server. |
| Tick controls | Provides tick-rate changes, freezing, stepping, sprinting through ticks, and timing queries. |
| Profiling and metrics | Offers supported debug reports, flight recording, and Java management metrics for inspecting server behavior. |
| Empty-server pause | Can suspend simulation after a configured period without players. |
| Logging and crash reports | Records startup, administrative, gameplay-related, and failure information and writes crash diagnostics. |
| Graceful shutdown | Stops accepting gameplay, disconnects players, saves applicable state, and closes server resources. |

References: [1.21.9 management API and code of conduct](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-21-9), [tick administration](https://www.minecraft.net/en-us/article/minecraft-java-edition-1-20-3), and the baseline release notes for management API 3.1.0. The [vanilla JMX introduction](https://feedback.minecraft.net/hc/en-us/articles/360042332451-Minecraft-Java-Edition-Snapshot-20W16A) describes tick metrics; Java tooling is an operational capability of the reference server, not a proposed dependency for Halcyon.
