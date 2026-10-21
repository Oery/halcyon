use super::*;

#[test]
fn deadlines_keep_20_steps_and_publications_per_second() {
    let start = Instant::now();
    let mut clock = SimulationClock::new(start);
    let mut steps = 0;
    let mut publications = 0;
    for _ in 0..SIMULATION_HZ {
        let deadline = clock.next_deadline();
        assert_eq!(clock.poll(deadline - Duration::from_nanos(1)).steps, 0);
        let advance = clock.poll(deadline);
        steps += advance.steps;
        publications += usize::from(advance.publication);
        assert_eq!(advance.skipped_steps, 0);
        assert_eq!(advance.lateness, Duration::ZERO);
    }
    assert_eq!(steps, 20);
    assert_eq!(publications, 20);
    assert_eq!(clock.next_deadline() - start, Duration::from_millis(1_050));
}

#[test]
fn overload_catches_up_with_bounded_work_and_one_publication() {
    let start = Instant::now();
    let mut clock = SimulationClock::new(start);
    let advance = clock.poll(start + Duration::from_secs(1));
    assert_eq!(advance.steps, 4);
    assert_eq!(advance.skipped_steps, 16);
    assert!(advance.publication);
    assert_eq!(advance.lateness, Duration::from_millis(950));
    assert_eq!(clock.poll(start + Duration::from_secs(1)), ClockAdvance::default());
    assert_eq!(clock.poll(clock.next_deadline()).steps, 1);
}
