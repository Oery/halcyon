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
fn static_bases_are_shared_immutable_and_support_duplicates_and_blank_lines() {
    static LAYOUT: SidebarLayout = SidebarLayout::new(
        "title",
        &[SidebarLine::new("same"), SidebarLine::new(""), SidebarLine::new("same")],
    );
    let mut board = board();
    let id = board.create_sidebar(Some(1), &LAYOUT).unwrap();
    let other = board.create_sidebar(Some(2), &LAYOUT).unwrap();
    let base = board.objectives[&id.serial].base.as_ref().unwrap().clone();
    assert!(Rc::ptr_eq(&base, board.objectives[&other.serial].base.as_ref().unwrap()));
    board.display_for(1, DisplaySlot::Sidebar, DisplayOverride::Objective(id)).unwrap();
    board.display_for(2, DisplaySlot::Sidebar, DisplayOverride::Objective(other)).unwrap();
    let entries = &base.scores;
    assert_eq!(entries.len(), 3);
    assert!(entries.values().all(|entry| !entry.holder.starts_with('#')));
    let mut observer = ScoreboardObserver::default();
    let initial = drain(&mut observer, &board, 1);
    assert_eq!(
        initial.iter().map(|frame| payload(frame).0).collect::<Vec<_>>(),
        [0x6C, 0x70, 0x70, 0x70, 0x64]
    );
    let mut second = ScoreboardObserver::default();
    let same_base = drain(&mut second, &board, 2);
    assert!(initial.iter().zip(&same_base).all(|(a, b)| Rc::ptr_eq(a, b) || payload(a).0 == 0x64));
    assert_eq!(board.update(id, ObjectiveSpec::default()), Err(ScoreboardError::ImmutableLayout));
    assert_eq!(
        board.reset_score(id, &ScoreHolder::Line(LineId::new(0))),
        Err(ScoreboardError::ImmutableLayout)
    );
    assert_eq!(
        board.display_for(1, DisplaySlot::BelowName, DisplayOverride::Objective(id)),
        Err(ScoreboardError::SidebarSlot)
    );
    board.update_line(id, LineId::new(0), format_args!("personal")).unwrap();
    assert_eq!(drain(&mut observer, &board, 1).len(), 1);
    assert!(drain(&mut second, &board, 2).is_empty());
    assert!(board.objectives[&other.serial].scores.is_empty());
    assert_eq!(base.scores[&ScoreHolder::Line(LineId::new(0))].revision, 0);
    assert_eq!(base.layout.lines[0].text, "same");
}

#[test]
fn a_timer_changes_one_line_without_rebuilding_or_sending_other_lines() {
    static LAYOUT: SidebarLayout = SidebarLayout::new(
        "Game",
        &[SidebarLine::new("Time: 60s"), SidebarLine::new(""), SidebarLine::new("Score: 10")],
    );
    let mut board = board();
    let id = board.create_sidebar(None, &LAYOUT).unwrap();
    board.display(DisplaySlot::Sidebar, Some(id)).unwrap();
    let mut observer = ScoreboardObserver::default();
    drain(&mut observer, &board, 1);
    let unchanged = board.objectives[&id.serial]
        .score(&ScoreHolder::Line(LineId::new(2)))
        .unwrap()
        .frame
        .clone();
    let definition = board.objectives[&id.serial].create.clone();
    board.update_line(id, LineId::new(0), format_args!("Time: {}s", 59)).unwrap();
    let frames = drain(&mut observer, &board, 1);
    assert_eq!(frames.len(), 1);
    let (packet, mut body) = payload(&frames[0]);
    assert_eq!(packet, 0x70);
    assert_eq!(<&str>::read(&mut body).unwrap(), "$halcyon:line_0");
    assert!(Rc::ptr_eq(
        &unchanged,
        &board.objectives[&id.serial].score(&ScoreHolder::Line(LineId::new(2))).unwrap().frame
    ));
    assert!(Rc::ptr_eq(&definition, &board.objectives[&id.serial].create));
    let revision = board.revision;
    for _ in 0..20 {
        board.update_line(id, LineId::new(0), format_args!("Time: 59s")).unwrap();
    }
    // Every explicit request publishes a new revision, without comparing text.
    // Slow writers still coalesce those requests into the latest encoded value.
    assert_eq!(board.revision, revision + 20);
    assert_eq!(drain(&mut observer, &board, 1).len(), 1);
    assert_eq!(
        board.update_line(id, LineId::new(3), format_args!("missing")),
        Err(ScoreboardError::InvalidLine)
    );
    assert_eq!(board.revision, revision + 20);
}

#[test]
fn explicit_updates_share_encoded_payloads_and_preserve_current_values_for_late_viewers() {
    static LAYOUT: SidebarLayout = SidebarLayout::new(
        "Game",
        &[SidebarLine::new("Time: 60s"), SidebarLine::new("Status: Waiting")],
    );
    let mut board = board();
    let first = board.create_sidebar(Some(1), &LAYOUT).unwrap();
    let second = board.create_sidebar(Some(2), &LAYOUT).unwrap();
    board.display_for(1, DisplaySlot::Sidebar, DisplayOverride::Objective(first)).unwrap();
    board.display_for(2, DisplaySlot::Sidebar, DisplayOverride::Objective(second)).unwrap();
    let mut slow = ScoreboardObserver::default();
    drain(&mut slow, &board, 1);
    board
        .update_lines([first, second].into_iter(), LineId::new(0), format_args!("Time: 59s"))
        .unwrap();
    let key = ScoreHolder::Line(LineId::new(0));
    assert!(Rc::ptr_eq(
        &board.objectives[&first.serial].score(&key).unwrap().frame,
        &board.objectives[&second.serial].score(&key).unwrap().frame
    ));
    let in_flight = slow.next_frame(&board, 1).unwrap().unwrap();
    for second_left in (30..59).rev() {
        board
            .update_lines(
                [first, second].into_iter(),
                LineId::new(0),
                format_args!("Time: {second_left}s"),
            )
            .unwrap();
    }
    assert!(Rc::ptr_eq(&in_flight, &slow.next_frame(&board, 1).unwrap().unwrap()));
    assert!(in_flight.windows(9).any(|w| w == b"Time: 59s"));
    slow.sent();
    let current = drain(&mut slow, &board, 1);
    assert_eq!(current.len(), 1);
    assert!(current[0].windows(9).any(|w| w == b"Time: 30s"));
    assert_eq!(board.objectives[&first.serial].scores.len(), 1);
    let mut late = ScoreboardObserver::default();
    let initial = drain(&mut late, &board, 2);
    assert_eq!(initial.iter().filter(|frame| payload(frame).0 == 0x70).count(), 2);
    assert!(initial.iter().any(|frame| frame.windows(9).any(|w| w == b"Time: 30s")));
    assert!(!initial.iter().any(|frame| frame.windows(9).any(|w| w == b"Time: 60s")));
    assert_eq!(
        board.objectives[&second.serial].base.as_ref().unwrap().layout.lines[0].text,
        "Time: 60s"
    );
}

#[test]
fn formatting_failures_and_invalid_batches_leave_published_state_untouched() {
    static LAYOUT: SidebarLayout = SidebarLayout::new("Game", &[SidebarLine::new("Score: 0")]);
    static OTHER: SidebarLayout = SidebarLayout::new("Other", &[SidebarLine::new("Score: 0")]);
    struct Fails;
    impl std::fmt::Display for Fails {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("partial")?;
            Err(std::fmt::Error)
        }
    }
    let mut board = board();
    let id = board.create_sidebar(None, &LAYOUT).unwrap();
    let other = board.create_sidebar(None, &OTHER).unwrap();
    let revision = board.revision;
    assert_eq!(
        board.update_line(id, LineId::new(0), format_args!("{}", Fails)),
        Err(ScoreboardError::InvalidText)
    );
    assert_eq!(
        board.update_line(id, LineId::new(0), format_args!("{:>5000}", "")),
        Err(ScoreboardError::InvalidText)
    );
    assert_eq!(
        board.update_lines([id, other].into_iter(), LineId::new(0), format_args!("Score: 10")),
        Err(ScoreboardError::InvalidLine)
    );
    assert_eq!(board.revision, revision);
    assert!(board.objectives[&id.serial].scores.is_empty());
    assert!(board.objectives[&other.serial].scores.is_empty());
}

#[test]
fn continuous_events_do_not_starve_other_objectives_or_initial_slot_assignment() {
    static LAYOUT: SidebarLayout = SidebarLayout::new("Game", &[SidebarLine::new("Ready")]);
    let mut board = board();
    let scores = objective(&mut board, None, "Scores");
    let sidebar = board.create_sidebar(None, &LAYOUT).unwrap();
    board.display(DisplaySlot::PlayerList, Some(scores)).unwrap();
    board.display(DisplaySlot::Sidebar, Some(sidebar)).unwrap();
    let mut observer = ScoreboardObserver::default();
    let mut displayed = [false; 2];
    for value in 0..40 {
        set(&mut board, scores, "points", value);
        if let Some(frame) = observer.next_frame(&board, 1).unwrap() {
            let (id, mut body) = payload(&frame);
            if id == 0x64 {
                displayed[VarInt::read(&mut body).unwrap().0 as usize] = true;
            }
            observer.sent();
        }
    }
    assert_eq!(displayed, [true, true]);
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
fn invalid_publications_and_capacity_limits_are_atomic_and_ids_are_scoped() {
    let mut board = board();
    let id = objective(&mut board, None, "title");
    set(&mut board, id, "keep", 42);
    let revision = board.revision;
    assert_eq!(
        board.update(
            id,
            ObjectiveSpec { title: Text::from("x".repeat(MAX_TEXT_BYTES)), ..Default::default() }
        ),
        Err(ScoreboardError::InvalidText)
    );
    let bad = Text(vec![Span {
        text: "bad".into(),
        style: Style { color: Some("unknown"), ..Default::default() },
    }]);
    assert_eq!(
        board.update(id, ObjectiveSpec { title: bad, ..Default::default() }),
        Err(ScoreboardError::InvalidText)
    );
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
