use super::*;

#[test]
fn directory_fills_the_fullest_open_instance_then_the_least_loaded_shard() {
    let directory = Directory::new(3);
    directory.publish(0, ShardSummary { players: 5, open: vec![("hub", 5)] });
    directory.publish(1, ShardSummary { players: 2, open: vec![("hub", 2)] });
    directory.publish(2, ShardSummary { players: 9, open: vec![("ffa", 3)] });
    assert_eq!(directory.online(), 16);
    assert_eq!(directory.least_loaded(), 1);
    assert_eq!(directory.fullest("hub", 1), Some((0, 5)));
    assert_eq!(directory.fullest("hub", 0), Some((1, 2)));
    assert_eq!(directory.fullest("ffa", 0), Some((2, 3)));
    assert_eq!(directory.fullest("ffa", 2), None);
    // Equally full instances go to the less-loaded shard.
    directory.publish(1, ShardSummary { players: 4, open: vec![("ffa", 3)] });
    assert_eq!(directory.fullest("ffa", 0), Some((1, 3)));
    // Unknown shards are ignored and never look idle.
    directory.publish(7, ShardSummary { players: 0, open: vec![("ffa", 8)] });
    assert_eq!(directory.load(7), usize::MAX);
    assert_eq!(directory.fullest("ffa", 0), Some((1, 3)));
}
