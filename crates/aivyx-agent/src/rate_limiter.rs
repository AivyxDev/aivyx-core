use std::collections::VecDeque;
use std::time::Instant;

use aivyx_core::{AivyxError, Result};

/// Token-bucket rate limiter that tracks calls within a sliding window.
///
/// Enforces a maximum number of operations per 60-second window.
pub struct RateLimiter {
    max_per_minute: u32,
    timestamps: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new(max_per_minute: u32) -> Self {
        Self {
            max_per_minute,
            timestamps: VecDeque::new(),
        }
    }

    /// Check if another operation is allowed. Returns `Ok(())` if within limits,
    /// or `Err(RateLimit)` if the limit would be exceeded.
    pub fn check(&mut self) -> Result<()> {
        self.prune();

        if self.timestamps.len() as u32 >= self.max_per_minute {
            return Err(AivyxError::RateLimit(format!(
                "exceeded {}/min limit",
                self.max_per_minute
            )));
        }

        self.timestamps.push_back(Instant::now());
        Ok(())
    }

    /// Number of calls made in the current window.
    pub fn current_count(&mut self) -> u32 {
        self.prune();
        self.timestamps.len() as u32
    }

    /// Remove timestamps older than 60 seconds.
    fn prune(&mut self) {
        let cutoff = Instant::now() - std::time::Duration::from_secs(60);
        while self.timestamps.front().is_some_and(|t| *t < cutoff) {
            self.timestamps.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_within_limit() {
        let mut limiter = RateLimiter::new(10);
        for _ in 0..10 {
            assert!(limiter.check().is_ok());
        }
    }

    #[test]
    fn rejects_over_limit() {
        let mut limiter = RateLimiter::new(3);
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_err());
    }

    #[test]
    fn current_count_tracks() {
        let mut limiter = RateLimiter::new(100);
        assert_eq!(limiter.current_count(), 0);
        limiter.check().unwrap();
        limiter.check().unwrap();
        assert_eq!(limiter.current_count(), 2);
    }
}
