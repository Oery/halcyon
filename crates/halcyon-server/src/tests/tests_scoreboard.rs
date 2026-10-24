use super::*;
use mc_protocol::types::VarInt;

fn board() -> Board {
    let mut board = Board::default();
    board.bind(InstanceId { shard: 0, local: 1 });
    board
}

fn objective(board: &mut Board, owner: Option<i32>, title: &str) -> ObjectiveId {
    board.create(owner, ObjectiveSpec { title: title.into(), ..Default::default() }).unwrap()
}

fn payload(frame: &[u8]) -> (i32, &[u8]) {
    let mut input = frame;
    assert_eq!(VarInt::read(&mut input).unwrap().0 as usize, input.len());
    let id = VarInt::read(&mut input).unwrap().0;
    (id, input)
}

fn drain(observer: &mut ScoreboardObserver, board: &Board, viewer: i32) -> Vec<Rc<[u8]>> {
    let mut frames = Vec::new();
    for _ in 0..100_000 {
        if let Some(frame) = observer.next_frame(board, viewer).unwrap() {
            frames.push(frame);
            observer.sent();
        } else if !observer.pending_work() {
            return frames;
        }
    }
    panic!("scoreboard did not converge");
}

fn set(board: &mut Board, id: ObjectiveId, key: &str, value: i32) {
    board
        .set_score(
            id,
            ScoreHolder::Entry(key.into()),
            Rc::from(format!("$halcyon:{key}")),
            Score { value, ..Default::default() },
        )
        .unwrap();
}

#[test]
fn personal_views_inherit_hide_and_never_publish_unselected_objectives() {
    let mut board = board();
    let shared = objective(&mut board, None, "shared");
    let private = objective(&mut board, Some(1), "secret");
    for slot in [DisplaySlot::Sidebar, DisplaySlot::PlayerList, DisplaySlot::BelowName] {
        board.display(slot, Some(shared)).unwrap();
    }
    board.display_for(1, DisplaySlot::Sidebar, DisplayOverride::Objective(private)).unwrap();
    assert_eq!(
        board.display_for(2, DisplaySlot::Sidebar, DisplayOverride::Objective(private)),
        Err(ScoreboardError::Audience)
    );
    assert_eq!(
        board.display(DisplaySlot::BelowName, Some(private)),
        Err(ScoreboardError::Audience)
    );
    let mut first = ScoreboardObserver::default();
    let mut second = ScoreboardObserver::default();
    let personal = drain(&mut first, &board, 1);
    let public = drain(&mut second, &board, 2);
    assert!(personal.iter().any(|frame| frame.windows(6).any(|w| w == b"secret")));
    assert!(!public.iter().any(|frame| frame.windows(6).any(|w| w == b"secret")));
    assert_eq!(public.iter().filter(|frame| payload(frame).0 == 0x64).count(), 3);
    board.display_for(1, DisplaySlot::Sidebar, DisplayOverride::Hidden).unwrap();
    let hidden = drain(&mut first, &board, 1);
    assert!(hidden.iter().any(|frame| payload(frame).0 == 0x6C));
    board.display_for(1, DisplaySlot::Sidebar, DisplayOverride::Inherit).unwrap();
    let inherited = drain(&mut first, &board, 1);
    assert_eq!(inherited.len(), 1);
    assert_eq!(payload(&inherited[0]).0, 0x64);
}

#[test]
fn sidebar_stable_keys_support_duplicates_blank_lines_and_no_op_updates() {
    let mut board = board();
    let id = objective(&mut board, None, "title");
    let lines = [Text::from("same"), Text::from(""), Text::from("same")];
    board.sidebar(id, "title".into(), &lines).unwrap();
    board.display(DisplaySlot::Sidebar, Some(id)).unwrap();
    let entries = &board.objectives[&id.serial].scores;
    assert_eq!(entries.len(), 3);
    assert_eq!(entries.values().map(|entry| entry.score.value).collect::<Vec<_>>(), [15, 14, 13]);
    assert!(entries.values().all(|entry| !entry.holder.starts_with('#')));
    let mut observer = ScoreboardObserver::default();
    let initial = drain(&mut observer, &board, 1);
    assert_eq!(
        initial.iter().map(|frame| payload(frame).0).collect::<Vec<_>>(),
        [0x6C, 0x70, 0x70, 0x70, 0x64]
    );
    let revision = board.revision;
    board.sidebar(id, "title".into(), &lines).unwrap();
    assert_eq!(board.revision, revision);
    assert!(drain(&mut observer, &board, 1).is_empty());
    board.sidebar(id, "new title".into(), &lines[..1]).unwrap();
    let changed = drain(&mut observer, &board, 1);
    assert_eq!(
        changed.iter().map(|frame| payload(frame).0).collect::<Vec<_>>(),
        [0x6C, 0x50, 0x50]
    );
    let mut body = payload(&changed[0]).1;
    <&str>::read(&mut body).unwrap();
    assert_eq!(u8::read(&mut body).unwrap(), 2);
}

#[test]
fn a_timer_changes_one_line_without_rebuilding_or_sending_other_lines() {
    let mut board = board();
    let id = objective(&mut board, None, "Game");
    board.sidebar(id, "Game".into(), &["Time: 60s".into(), "".into(), "Score: 10".into()]).unwrap();
    board.display(DisplaySlot::Sidebar, Some(id)).unwrap();
    let mut observer = ScoreboardObserver::default();
    drain(&mut observer, &board, 1);
    let unchanged =
        board.objectives[&id.serial].scores[&ScoreHolder::Entry("line_2".into())].frame.clone();
    let definition = board.objectives[&id.serial].create.clone();
    board.set_sidebar_line(id, 0, "Time: 59s".into()).unwrap();
    let frames = drain(&mut observer, &board, 1);
    assert_eq!(frames.len(), 1);
    let (packet, mut body) = payload(&frames[0]);
    assert_eq!(packet, 0x70);
    assert_eq!(<&str>::read(&mut body).unwrap(), "$halcyon:line_0");
    assert!(Rc::ptr_eq(
        &unchanged,
        &board.objectives[&id.serial].scores[&ScoreHolder::Entry("line_2".into())].frame
    ));
    assert!(Rc::ptr_eq(&definition, &board.objectives[&id.serial].create));
    let revision = board.revision;
    for _ in 0..20 {
        board.set_sidebar_line(id, 0, "Time: 59s".into()).unwrap();
    }
    assert_eq!(board.revision, revision);
    assert!(drain(&mut observer, &board, 1).is_empty());
    assert_eq!(board.set_sidebar_line(id, 3, "missing".into()), Err(ScoreboardError::InvalidLine));
    assert_eq!(board.set_sidebar_line(id, 15, "outside".into()), Err(ScoreboardError::InvalidLine));
    assert_eq!(board.revision, revision);
}

#[test]
fn stalled_writer_commits_only_sent_frames_and_converges_to_latest_scores() {
    let mut board = board();
    let id = objective(&mut board, None, "title");
    board.display(DisplaySlot::PlayerList, Some(id)).unwrap();
    set(&mut board, id, "points", 1);
    let mut observer = ScoreboardObserver::default();
    let first = observer.next_frame(&board, 1).unwrap().unwrap();
    for value in 2..100 {
        set(&mut board, id, "points", value);
        assert!(Rc::ptr_eq(&first, &observer.next_frame(&board, 1).unwrap().unwrap()));
    }
    observer.sent();
    let frames = drain(&mut observer, &board, 1);
    let scores: Vec<_> = frames.iter().filter(|frame| payload(frame).0 == 0x70).collect();
    assert_eq!(scores.len(), 1);
    let mut body = payload(scores[0]).1;
    <&str>::read(&mut body).unwrap();
    <&str>::read(&mut body).unwrap();
    assert_eq!(VarInt::read(&mut body).unwrap().0, 99);
    board.reset_score(id, &ScoreHolder::Entry("points".into())).unwrap();
    assert_eq!(payload(&drain(&mut observer, &board, 1)[0]).0, 0x50);
}

#[test]
fn score_capacity_and_churn_keep_owner_and_delivered_state_bounded() {
    let mut board = board();
    let id = objective(&mut board, None, "scores");
    board.display(DisplaySlot::PlayerList, Some(id)).unwrap();
    for n in 0..MAX_SCORES {
        set(&mut board, id, &format!("{n:05}"), n as i32);
    }
    assert_eq!(
        board.set_score(
            id,
            ScoreHolder::Entry("overflow".into()),
            Rc::from("$overflow"),
            Score::default()
        ),
        Err(ScoreboardError::Capacity)
    );
    let mut observer = ScoreboardObserver::default();
    drain(&mut observer, &board, 1);
    set(&mut board, id, "00001", 77);
    // Finish the removal phase and start delivering an update, then delete a
    // key behind the cursor while adding one ahead of it at maximum capacity.
    loop {
        if observer.next_frame(&board, 1).unwrap().is_some() {
            observer.sent();
            break;
        }
    }
    board.reset_score(id, &ScoreHolder::Entry("00000".into())).unwrap();
    set(&mut board, id, "zzzzz", 78);
    let frames = drain(&mut observer, &board, 1);
    assert!(frames.iter().any(|frame| {
        let (packet, mut body) = payload(frame);
        if packet != 0x6C {
            return false;
        }
        <&str>::read(&mut body).unwrap();
        u8::read(&mut body).unwrap() == 1
    }));
    assert_eq!(board.score_count, MAX_SCORES);
    assert!(drain(&mut observer, &board, 1).is_empty());
}

#[test]
fn departure_and_transfer_cleanup_remove_only_delivered_state() {
    let mut board = board();
    let shared = objective(&mut board, None, "shared");
    let personal = objective(&mut board, Some(1), "personal");
    board.set_score(shared, ScoreHolder::Player(1), Rc::from("Alice"), Score::default()).unwrap();
    board.display(DisplaySlot::BelowName, Some(shared)).unwrap();
    board.display_for(1, DisplaySlot::Sidebar, DisplayOverride::Objective(personal)).unwrap();
    let mut observer = ScoreboardObserver::default();
    drain(&mut observer, &board, 1);
    board.player_left(1);
    assert!(!board.objectives.contains_key(&personal.serial));
    assert!(!board.viewers.contains_key(&1));
    assert_eq!(board.score_count, 0);
    let frames = drain(&mut observer, &board, 1);
    assert!(frames.iter().any(|frame| payload(frame).0 == 0x50));
    let cleanup = observer.clear().unwrap();
    assert_eq!(cleanup.iter().map(|frame| payload(frame).0).collect::<Vec<_>>(), [0x64, 0x6C]);
    assert!(!observer.pending_work());
    assert!(observer.clear().unwrap().is_empty());
    // An unsent definition must not be removed as if the client knew it.
    observer.next_frame(&board, 2).unwrap().unwrap();
    assert!(observer.clear().unwrap().is_empty());
    assert!(!drain(&mut observer, &board, 2).is_empty());
}

#[test]
fn invalid_replacements_and_capacity_limits_are_atomic_and_ids_are_scoped() {
    let mut board = board();
    let id = objective(&mut board, None, "title");
    set(&mut board, id, "keep", 42);
    let revision = board.revision;
    assert_eq!(
        board.sidebar(id, "title".into(), &vec![Text::from("x"); 16]),
        Err(ScoreboardError::Capacity)
    );
    assert_eq!(
        board.sidebar(id, Text::from("x".repeat(MAX_TEXT_BYTES)), &[]),
        Err(ScoreboardError::InvalidText)
    );
    let bad = Text(vec![Span {
        text: "bad".into(),
        style: Style { color: Some("unknown".into()), ..Default::default() },
    }]);
    assert_eq!(board.sidebar(id, "title".into(), &[bad]), Err(ScoreboardError::InvalidText));
    assert_eq!(board.revision, revision);
    assert_eq!(board.score_count, 1);
    let foreign = ObjectiveId { instance: InstanceId { shard: 1, local: 1 }, ..id };
    assert_eq!(board.remove(foreign), Err(ScoreboardError::Objective));
    for _ in 1..MAX_OBJECTIVES {
        objective(&mut board, None, "x");
    }
    let revision = board.revision;
    assert_eq!(board.create(None, ObjectiveSpec::default()), Err(ScoreboardError::Capacity));
    assert_eq!(board.revision, revision);
    board.remove(id).unwrap();
    assert_eq!(board.update(id, ObjectiveSpec::default()), Err(ScoreboardError::Objective));
    assert_ne!(objective(&mut board, None, "new"), id);
}

#[test]
fn large_boards_resume_bounded_scans_and_preserve_updates_during_a_pass() {
    let mut board = board();
    let id = objective(&mut board, None, "scores");
    board.display(DisplaySlot::PlayerList, Some(id)).unwrap();
    for n in 0..600 {
        set(&mut board, id, &format!("{n:04}"), n);
    }
    let mut observer = ScoreboardObserver::default();
    assert_eq!(drain(&mut observer, &board, 1).len(), 602);
    // Changing one late key requires scanning unchanged entries, yielding first.
    set(&mut board, id, "0599", 900);
    assert!(observer.next_frame(&board, 1).unwrap().is_none());
    assert!(observer.pending_work());
    set(&mut board, id, "0000", 901);
    let frames = drain(&mut observer, &board, 1);
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|frame| payload(frame).0 == 0x70));
}
