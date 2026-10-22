use super::*;

#[derive(Debug, PartialEq, Eq, CommandValue)]
#[value(into = native::Mode)]
enum Mode {
    #[value(alias = "0")]
    Survival,
    #[value(name = "creative", alias = "1")]
    Build,
}

mod native {
    #[derive(Debug, PartialEq, Eq)]
    pub enum Mode {
        Survival,
        Build,
    }
}

#[derive(Debug, CommandArgs)]
struct Select<'p> {
    mode: Mode,
    #[arg(name = "player")]
    target: Option<PlayerName<'p>>,
}

#[derive(Debug, CommandArgs)]
struct Numbers {
    count: i32,
    weight: f32,
    big: Option<i64>,
    ratio: Option<f64>,
}

#[derive(Debug, CommandArgs)]
struct Help;

#[derive(Debug, CommandSet)]
enum Commands<'p> {
    #[command(name = "select")]
    Select(Select<'p>),
    Numbers(Numbers),
    Help(Help),
}

#[test]
fn derives_parse_borrowed_choices_and_typed_numbers_with_trailing_optionals() {
    let text = String::from("select 1 Alex");
    let Commands::Select(args) = Commands::parse(&text).unwrap() else {
        panic!("wrong command")
    };
    assert_eq!(args.mode, Mode::Build);
    assert_eq!(native::Mode::from(args.mode), native::Mode::Build);
    let name = args.target.unwrap().as_str();
    assert_eq!(name, "Alex");
    assert_eq!(name.as_ptr(), text[9..].as_ptr());
    let Commands::Select(args) = Commands::parse("select 0").unwrap() else {
        panic!("wrong command")
    };
    assert_eq!(args.mode, Mode::Survival);
    assert_eq!(native::Mode::from(args.mode), native::Mode::Survival);
    assert!(args.target.is_none());
    let Commands::Numbers(args) =
        Commands::parse("numbers -4 0.5 9223372036854775807 1.25").unwrap()
    else {
        panic!("wrong command")
    };
    assert_eq!(
        (args.count, args.weight, args.big, args.ratio),
        (-4, 0.5, Some(i64::MAX), Some(1.25))
    );
    let Commands::Numbers(args) = Commands::parse("numbers 4 0.5").unwrap() else {
        panic!("wrong command")
    };
    assert_eq!((args.big, args.ratio), (None, None));
    assert!(matches!(Commands::parse("help").unwrap(), Commands::Help(_)));
    for text in [
        "numbers 2147483648 1",
        "numbers 1 NaN",
        "numbers 1 inf",
        "numbers +1 0",
        "numbers 1 1e3",
        "numbers 1 99999999999999999999999999999999999999999999999999",
        "numbers 1 0 9223372036854775808",
        "numbers 1 0 2 NaN",
    ] {
        assert!(matches!(
            Commands::parse(text),
            Err(ParseError::Arguments { error: ArgumentError::Invalid { .. }, .. })
        ));
    }
    assert!(matches!(
        Commands::parse("select"),
        Err(ParseError::Arguments { error: ArgumentError::Missing("mode"), .. })
    ));
    assert!(matches!(
        Commands::parse("help extra"),
        Err(ParseError::Arguments { error: ArgumentError::Extra, .. })
    ));
    assert!(matches!(Commands::parse("missing"), Err(ParseError::UnknownCommand)));
    assert_eq!(Commands::COMMANDS[0].usage(), "/select <survival|0|creative|1> [player]");
    assert!(Commands::parse("select bad").unwrap_err().to_string().contains("Expected one of:"));
    assert!(PlayerName::parse("@a").is_err());
}

struct Players;

impl CompletionContext for Players {
    fn suggest(&self, provider: &str, prefix: &str) -> Vec<String> {
        assert_eq!(provider, "players");
        ["Alex", "Alice"]
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn derived_completion_validates_prior_arguments_and_uses_application_providers() {
    assert_eq!(Commands::complete("/", &Players).matches, ["help", "numbers", "select"]);
    assert_eq!(Commands::complete("/select cr", &Players).matches, ["creative"]);
    let completion = Commands::complete("/select 1 Al", &Players);
    assert_eq!(completion.matches, ["Alex", "Alice"]);
    assert_eq!((completion.start, completion.length), (10, 2));
    assert!(Commands::complete("/select invalid Al", &Players).matches.is_empty());
    assert!(Commands::complete("/select 1 Alex extra", &Players).matches.is_empty());
    assert!(Commands::complete("/help ", &Players).matches.is_empty());
    let completion = Commands::complete("/select\u{2003}😀", &Players);
    assert_eq!((completion.start, completion.length), (8, 2));
    assert!(completion.matches.is_empty());
}

#[test]
fn derived_tree_shares_choice_suffixes_and_marks_only_complete_commands_executable() {
    let tree = Commands::tree();
    let root = &tree.nodes[tree.root];
    assert_eq!(root.children.len(), 3);
    assert_eq!(tree.nodes.len(), 13);
    for (index, node) in tree.nodes.iter().enumerate() {
        assert!(node.children.iter().all(|&child| child < index));
    }
    let select = &tree.nodes[root.children[0]];
    assert!(!select.executable);
    assert_eq!(select.children.len(), 4);
    let choices: Vec<_> = select.children.iter().map(|&id| &tree.nodes[id]).collect();
    assert!(choices.iter().all(|node| node.executable && node.children == choices[0].children));
    let target = &tree.nodes[choices[0].children[0]];
    assert!(target.executable);
    assert_eq!(
        target.kind,
        NodeKind::Argument(ArgumentSpec {
            name: "player",
            optional: true,
            kind: ArgumentKind::Word { suggestions: Some("players") }
        })
    );
    let numbers = &tree.nodes[root.children[1]];
    assert!(!numbers.executable);
    let count = &tree.nodes[numbers.children[0]];
    assert!(!count.executable);
    let weight = &tree.nodes[count.children[0]];
    assert!(weight.executable);
    assert!(tree.nodes[root.children[2]].executable);
}
