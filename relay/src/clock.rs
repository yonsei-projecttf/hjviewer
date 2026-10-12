// SPDX-License-Identifier: AGPL-3.0-only

//! The relay's estimate of the server's clock: the last `server_time` the server sent plus the time
//! since then on the monotonic clock. Command expiry and request timestamps use it, so they do not
//! depend on the NAS's own clock (PROTOCOL.md, "Time").

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// `server_time` values outside this range (2020 to 3000) are ignored as nonsense.
const PLAUSIBLE: std::ops::RangeInclusive<i64> = 1_577_836_800..=32_503_680_000;

#[derive(Clone, Debug, Default)]
pub struct ServerClock {
    base: Option<(i64, Instant)>,
}

impl ServerClock {
    pub fn new() -> ServerClock {
        ServerClock::default()
    }

    /// Takes `server_time` as the server's time now.
    pub fn update(&mut self, server_time: i64) {
        self.update_at(server_time, Instant::now());
    }

    pub fn update_at(&mut self, server_time: i64, at: Instant) {
        if PLAUSIBLE.contains(&server_time) {
            self.base = Some((server_time, at));
        }
    }

    /// The estimated server time in Unix seconds; the local clock until the server has answered.
    pub fn now(&self) -> i64 {
        self.now_at(Instant::now())
    }

    pub fn now_at(&self, at: Instant) -> i64 {
        match self.base {
            Some((server_time, base)) => {
                let elapsed = at.saturating_duration_since(base).as_secs();
                server_time.saturating_add(i64::try_from(elapsed).unwrap_or(i64::MAX))
            }
            None => system_now(),
        }
    }

    pub fn synced(&self) -> bool {
        self.base.is_some()
    }
}

/// The local clock in Unix seconds.
pub fn system_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn follows_the_server_on_the_monotonic_clock() {
        let mut clock = ServerClock::new();
        assert!(!clock.synced());
        assert!((clock.now() - system_now()).abs() <= 1);
        let t0 = Instant::now();
        clock.update_at(1_760_000_000, t0);
        assert!(clock.synced());
        assert_eq!(clock.now_at(t0), 1_760_000_000);
        assert_eq!(
            clock.now_at(t0 + Duration::from_millis(59_999)),
            1_760_000_059
        );
        assert_eq!(clock.now_at(t0 + Duration::from_secs(3600)), 1_760_003_600);
        // nonsense is ignored
        clock.update_at(5, t0);
        clock.update_at(i64::MAX, t0);
        assert_eq!(clock.now_at(t0), 1_760_000_000);
    }
}
