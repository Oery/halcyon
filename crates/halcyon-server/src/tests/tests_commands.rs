use super::*;
use halcyon_commands::{ArgumentError, ParseError};
use mc_protocol::types::GameMode;

#[test]
fn basic_give_accepts_plain_items_and_completes_only_the_supported_catalog() {
    assert!(matches!(Command::parse("give minecraft:stone 4").unwrap(), Command::Give(_)));
    assert!(matches!(Command::parse("give oak_sign").unwrap(), Command::Give(_)));
    assert!(Command::parse("give stone[foo=bar]").is_err());
    assert!(Command::parse("give stone 1 Player").is_err());
    assert_eq!(complete("/give diamond_pick", &Registry::default()).matches, ["diamond_pickaxe"]);
    assert!(complete("/give water_buck", &Registry::default()).matches.is_empty());
}

#[test]
fn gamemode_parses_names_numeric_aliases_and_optional_targets() {
    for (name, id) in [
        ("survival", 0),
        ("creative", 1),
        ("adventure", 2),
        ("spectator", 3),
        ("0", 0),
        ("1", 1),
        ("2", 2),
        ("3", 3),
    ] {
        let text = format!("gamemode {name}");
        let Command::GameMode(command) = Command::parse(&text).unwrap() else {
            panic!("wrong command")
        };
        assert_eq!(GameMode::from(command.mode), GameMode::try_from(id).unwrap());
        assert_eq!(command.target, None);
        let text = format!("gamemode {name} Player_1");
        let Command::GameMode(command) = Command::parse(&text).unwrap() else {
            panic!("wrong command")
        };
        assert_eq!(command.target.map(halcyon_commands::PlayerName::as_str), Some("Player_1"));
    }
    for text in
        ["gamemode", "gamemode 1 Player extra", "gamemode 1 @a", "gamemode 1 abcdefghijklmnopq"]
    {
        assert!(matches!(Command::parse(text), Err(ParseError::Arguments { .. })));
    }
    for text in ["gamemode 4", "gamemode -1", "gamemode Creative", "gamemode invalid"] {
        assert!(matches!(
            Command::parse(text),
            Err(ParseError::Arguments { error: ArgumentError::Invalid { name: "mode", .. }, .. })
        ));
    }
    assert!(matches!(Command::parse("unknown 1"), Err(ParseError::UnknownCommand)));
    assert!(matches!(Command::parse("play ffa").unwrap(), Command::Play(_)));
    assert!(matches!(Command::parse("hub").unwrap(), Command::Hub(_)));
    assert!(Command::parse("play chess").is_err());
}

#[test]
fn advertised_tree_matches_execution_syntax() {
    let tree = tree();
    let root = &tree.nodes[tree.root_index as usize];
    assert_eq!(root.kind, CommandNodeKind::Root);
    assert_eq!(root.children.len(), 5);
    let command = &tree.nodes[root.children[0].0 as usize];
    assert_eq!(command.kind, CommandNodeKind::Literal("gamemode"));
    assert_eq!(command.children.len(), 8);
    assert!(!command.executable);
    for (index, node) in tree.nodes.iter().enumerate() {
        assert!(node.children.iter().all(|child| child.0 >= 0 && (child.0 as usize) < index));
        if let CommandNodeKind::Literal(name) = node.kind
            && command.children.iter().any(|child| child.0 == index as i32)
        {
            assert!(Command::parse(&format!("gamemode {name}")).is_ok());
            assert!(node.executable);
            let target = &tree.nodes[node.children[0].0 as usize];
            assert_eq!(target.kind, CommandNodeKind::Word { name: "player", suggest: true });
            assert!(target.executable);
        }
    }
    let tp = &tree.nodes[root.children[1].0 as usize];
    assert_eq!(tp.kind, CommandNodeKind::Literal("tp"));
    assert_eq!(tp.children.len(), 3); // coordinates, destination player, target player
    let targets: Vec<_> = tp
        .children
        .iter()
        .map(|child| &tree.nodes[child.0 as usize])
        .filter(|node| matches!(node.kind, CommandNodeKind::Word { name: "target", .. }))
        .collect();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].children.len(), 2);
    assert!(!targets[0].executable);
}

#[test]
fn tp_derives_all_four_forms_and_world_coordinate_parsing() {
    use tp::TpCommand;
    let Command::Teleport(TpCommand::Player(args)) = Command::parse("tp Alex").unwrap() else {
        panic!("wrong tp form")
    };
    assert_eq!(args.destination.as_str(), "Alex");
    let Command::Teleport(TpCommand::TargetPlayer(args)) = Command::parse("tp Alex Steve").unwrap()
    else {
        panic!("wrong tp form")
    };
    assert_eq!((args.target.as_str(), args.destination.as_str()), ("Alex", "Steve"));
    let Command::Teleport(TpCommand::Position(args)) = Command::parse("tp -1 64 2").unwrap() else {
        panic!("wrong tp form")
    };
    assert_eq!(args.location.resolve([8.5, 64.0, 8.5]), [-0.5, 64.0, 2.5]);
    let Command::Teleport(TpCommand::TargetPosition(args)) =
        Command::parse("tp Alex ~1 70.0 ~-2.25").unwrap()
    else {
        panic!("wrong tp form")
    };
    assert_eq!(args.target.as_str(), "Alex");
    assert_eq!(args.location.resolve([8.5, 64.0, 8.5]), [9.5, 70.0, 6.25]);
    for input in [
        "tp",
        "tp ~ ~",
        "tp Alex 1 2",
        "tp @a Alex",
        "tp 1 NaN 3",
        "tp ^ ^ ^",
        "tp Alex Steve extra",
    ] {
        assert!(Command::parse(input).is_err(), "{input}");
    }
    assert!(Command::COMMANDS[1].usage().contains("/tp <target> <x y z>"));
    assert_eq!(complete("/t", &Registry::default()).matches, ["tp"]);
    assert_eq!(complete("/tp ~ ", &Registry::default()).matches, ["~"]);
    assert_eq!(complete("/tp Alex ~ ", &Registry::default()).matches, ["~"]);
    assert!(complete("/tp ~ ~ ~ extra", &Registry::default()).matches.is_empty());
}

#[test]
fn completion_uses_utf16_offsets_and_stops_after_supported_arguments() {
    let registry = Registry::default();
    let result = complete("/gam", &registry);
    assert_eq!((result.start, result.length), (1, 3));
    assert_eq!(result.matches, ["gamemode"]);
    let result = complete("/gamemode cre", &registry);
    assert_eq!((result.start, result.length), (10, 3));
    assert_eq!(result.matches, ["creative"]);
    assert_eq!(complete("/gamemode 1", &registry).matches, ["1"]);
    assert_eq!(complete("/gamemode ", &registry).matches.len(), 8);
    let result = complete("/gamemode\u{2003}😀", &registry);
    assert_eq!((result.start, result.length), (10, 2));
    assert!(result.matches.is_empty());
    assert!(complete("/gamemode 1 Player extra", &registry).matches.is_empty());
    assert!(complete("/unknown ", &registry).matches.is_empty());
}
