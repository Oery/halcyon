use crate::error::Error;
use std::time::{Duration, Instant};

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LoginProgress {
    #[default]
    Start,
    Encryption,
    Acknowledge,
    Complete,
}

impl LoginProgress {
    pub fn advance(&mut self, expected: Self, next: Self) -> Result<(), Error> {
        if *self != expected {
            return Err(Error::UnexpectedResponse);
        }
        *self = next;
        Ok(())
    }
}

pub struct Keepalive {
    next: Instant,
    pending: Option<(i64, Instant)>,
    id: i64,
}

impl Keepalive {
    pub fn new(now: Instant) -> Self {
        Self { next: now, pending: None, id: 0 }
    }

    pub fn deadline(&self) -> Instant {
        self.pending.map_or(self.next, |(_, sent)| sent + KEEPALIVE_TIMEOUT)
    }

    pub fn response_deadline(&self) -> Option<Instant> {
        self.pending.map(|(_, sent)| sent + KEEPALIVE_TIMEOUT)
    }

    pub fn tick(&mut self, now: Instant) -> Result<Option<i64>, Error> {
        if let Some((_, sent)) = self.pending {
            if now >= sent + KEEPALIVE_TIMEOUT {
                return Err(Error::Timeout);
            }
        } else if now >= self.next {
            self.id = self.id.wrapping_add(1);
            self.pending = Some((self.id, now));
            self.next = now + KEEPALIVE_INTERVAL;
            return Ok(Some(self.id));
        }
        Ok(None)
    }

    pub fn acknowledge(&mut self, id: i64) -> Result<(), Error> {
        if !matches!(self.pending, Some((expected, _)) if expected == id) {
            return Err(Error::UnexpectedResponse);
        }
        self.pending = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premature_and_repeated_login_packets_are_rejected() {
        let mut progress = LoginProgress::default();
        assert!(progress.advance(LoginProgress::Acknowledge, LoginProgress::Complete).is_err());
        progress.advance(LoginProgress::Start, LoginProgress::Encryption).unwrap();
        assert!(progress.advance(LoginProgress::Start, LoginProgress::Encryption).is_err());
        progress.advance(LoginProgress::Encryption, LoginProgress::Acknowledge).unwrap();
        progress.advance(LoginProgress::Acknowledge, LoginProgress::Complete).unwrap();
    }

    #[test]
    fn idle_keepalives_have_independent_deadlines() {
        let now = Instant::now();
        let mut keepalive = Keepalive::new(now);
        let id = keepalive.tick(now).unwrap().unwrap();
        assert!(keepalive.acknowledge(id + 1).is_err());
        assert!(keepalive.tick(now + KEEPALIVE_INTERVAL).unwrap().is_none());
        assert!(keepalive.tick(now + KEEPALIVE_TIMEOUT).is_err());
        keepalive.acknowledge(id).unwrap();
        assert!(keepalive.tick(now + KEEPALIVE_INTERVAL).unwrap().is_some());
    }
}
