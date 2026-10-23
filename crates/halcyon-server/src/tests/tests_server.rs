use super::*;

#[test]
fn directory_places_games_on_open_then_least_loaded_shards() {
    let directory = Directory::new(3);
    directory.publish(0, ShardSummary { players: 5, open: vec![] });
    directory.publish(1, ShardSummary { players: 2, open: vec![] });
    directory.publish(2, ShardSummary { players: 9, open: vec!["ffa"] });
    assert_eq!(directory.online(), 16);
    assert_eq!(directory.least_loaded(), 1);
    assert_eq!(directory.open("ffa"), Some(2));
    assert_eq!(directory.open("hub"), None);
    directory.publish(0, ShardSummary { players: 5, open: vec!["ffa"] });
    assert_eq!(directory.open("ffa"), Some(0));
    // Unknown shards are ignored and never look idle.
    directory.publish(7, ShardSummary { players: 0, open: vec!["ffa"] });
    assert_eq!(directory.load(7), usize::MAX);
    assert_eq!(directory.open("ffa"), Some(0));
}
