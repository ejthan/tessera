// SPDX-License-Identifier: GPL-3.0-or-later

//! NF4 spans: synchronous work on the UI thread that should fit in a frame. A span over budget
//! warns and never panics, because a debug frame takes longer than 16 ms by itself.

use std::time::{Duration, Instant};

/// One frame at 60 Hz.
pub const BUDGET: Duration = Duration::from_millis(16);

/// Whether a span of `elapsed` warns.
pub fn over_budget(elapsed: Duration) -> bool {
    elapsed > BUDGET
}

/// Times its own lifetime; see [`span`].
pub struct Budget {
    name: &'static str,
    started: Instant,
}

/// Starts a span named `name`. Dropping it logs at warn, target `tessera::nf4`, above [`BUDGET`].
#[must_use]
pub fn span(name: &'static str) -> Budget {
    Budget {
        name,
        started: Instant::now(),
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        if over_budget(elapsed) {
            tracing::warn!(target: "tessera::nf4", name = self.name, ms = elapsed.as_millis());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_past_the_budget_counts() {
        assert!(!over_budget(BUDGET - Duration::from_millis(1)));
        assert!(!over_budget(BUDGET));
        assert!(over_budget(BUDGET + Duration::from_millis(1)));
    }
}
