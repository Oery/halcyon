//! Fixed simulation deadlines and independently paced network publication.
use std::time::{Duration, Instant};

pub const SIMULATION_HZ: u64 = 120;
pub const MAX_CATCH_UP_STEPS: usize = 4;
const PUBLICATION_INTERVAL: Duration = Duration::from_millis(50);
const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockAdvance {
    pub steps: usize,
    pub skipped_steps: u64,
    pub publication: bool,
    /// Delay since the oldest outstanding simulation deadline, before catch-up.
    pub lateness: Duration,
}

pub struct SimulationClock {
    origin: Instant,
    completed: u64,
    publication: u64,
}

impl SimulationClock {
    pub fn new(now: Instant) -> Self {
        Self { origin: now, completed: 0, publication: 0 }
    }

    pub fn next_deadline(&self) -> Instant {
        // Ceiling division prevents a deadline firing before its rational tick.
        let nanos = ((u128::from(self.completed) + 1) * NANOS_PER_SECOND)
            .div_ceil(u128::from(SIMULATION_HZ));
        self.origin + Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64)
    }

    pub fn poll(&mut self, now: Instant) -> ClockAdvance {
        let elapsed = now.saturating_duration_since(self.origin);
        let due = (elapsed.as_nanos() * u128::from(SIMULATION_HZ) / NANOS_PER_SECOND)
            .min(u128::from(u64::MAX)) as u64;
        let pending = due.saturating_sub(self.completed);
        let steps = pending.min(MAX_CATCH_UP_STEPS as u64) as usize;
        let lateness = if pending == 0 {
            Duration::ZERO
        } else {
            now.saturating_duration_since(self.next_deadline())
        };
        self.completed = self.completed.max(due);
        let publication =
            (elapsed.as_nanos() / PUBLICATION_INTERVAL.as_nanos()).min(u128::from(u64::MAX)) as u64;
        let publish = publication > self.publication;
        self.publication = self.publication.max(publication);
        ClockAdvance {
            steps,
            skipped_steps: pending.saturating_sub(steps as u64),
            publication: publish,
            lateness,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_deadlines_keep_120_steps_and_20_publications_per_second() {
        let start = Instant::now();
        let mut clock = SimulationClock::new(start);
        let mut steps = 0;
        let mut publications = 0;
        for _ in 0..120 {
            let deadline = clock.next_deadline();
            assert_eq!(clock.poll(deadline - Duration::from_nanos(1)).steps, 0);
            let advance = clock.poll(deadline);
            steps += advance.steps;
            publications += usize::from(advance.publication);
            assert_eq!(advance.skipped_steps, 0);
            assert_eq!(advance.lateness, Duration::ZERO);
        }
        assert_eq!(steps, 120);
        assert_eq!(publications, 20);
        assert_eq!(clock.next_deadline() - start, Duration::from_nanos(1_008_333_334));
    }

    #[test]
    fn overload_catches_up_with_bounded_work_and_one_publication() {
        let start = Instant::now();
        let mut clock = SimulationClock::new(start);
        let advance = clock.poll(start + Duration::from_secs(1));
        assert_eq!(advance.steps, 4);
        assert_eq!(advance.skipped_steps, 116);
        assert!(advance.publication);
        assert!(advance.lateness > Duration::from_millis(990));
        assert_eq!(clock.poll(start + Duration::from_secs(1)), ClockAdvance::default());
        assert_eq!(clock.poll(clock.next_deadline()).steps, 1);
    }
}
