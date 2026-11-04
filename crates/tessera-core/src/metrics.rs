// SPDX-License-Identifier: GPL-3.0-or-later

//! Time-windowed history of a node's `/metrics.json` samples (spec §6.4).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tessera_client::Metrics;

/// A chartable metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Series {
    /// System CPU use in percent.
    Cpu,
    /// System memory use as a percentage of the total.
    Memory,
    /// Disk use as a percentage of the total.
    Disk,
    /// Network bytes received per second.
    NetRx,
    /// Network bytes sent per second.
    NetTx,
}

impl Series {
    fn index(self) -> usize {
        self as usize
    }
}

struct Sample {
    at: Instant,
    values: [Option<f64>; 5],
}

/// Samples of one node, kept for a fixed time window.
pub struct MetricsHistory {
    window: Duration,
    samples: VecDeque<Sample>,
    /// Cumulative counters and time of the previous push, for rates.
    prev: Option<(Instant, Option<u64>, Option<u64>)>,
    last: Option<Metrics>,
    revision: u64,
}

fn percent(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(u), Some(t)) if t > 0 => Some(u as f64 / t as f64 * 100.0),
        _ => None,
    }
}

/// Bytes per second between two cumulative counters; `None` on a reset or no elapsed time.
fn rate(prev: Option<u64>, now: Option<u64>, secs: f64) -> Option<f64> {
    match (prev, now) {
        (Some(p), Some(n)) if n >= p && secs > 0.0 => Some((n - p) as f64 / secs),
        _ => None,
    }
}

impl MetricsHistory {
    /// Creates an empty history that keeps samples for `window` (15 minutes in the app).
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            samples: VecDeque::new(),
            prev: None,
            last: None,
            revision: 0,
        }
    }

    /// Adds a sample taken at `at` and drops samples older than the window, measured from `at`.
    pub fn push(&mut self, at: Instant, m: &Metrics) {
        let mut values = [None; 5];
        values[Series::Cpu.index()] = m.cpu_percent;
        values[Series::Memory.index()] = percent(m.memory_used, m.memory_total);
        values[Series::Disk.index()] = percent(m.disk_used, m.disk_total);
        if let Some((pat, prx, ptx)) = self.prev {
            let secs = at.saturating_duration_since(pat).as_secs_f64();
            values[Series::NetRx.index()] = rate(prx, m.net_received, secs);
            values[Series::NetTx.index()] = rate(ptx, m.net_sent, secs);
        }
        self.prev = Some((at, m.net_received, m.net_sent));
        self.samples.push_back(Sample { at, values });
        while let Some(front) = self.samples.front() {
            if at.saturating_duration_since(front.at) > self.window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.last = Some(m.clone());
        self.revision += 1;
    }

    /// Changes with every push, so a chart can rebuild only when the history changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Points of `series`, oldest first, as (age relative to the newest sample, value).
    /// Samples without a value for the series are skipped.
    pub fn points(&self, series: Series) -> impl Iterator<Item = (Duration, f64)> + '_ {
        let newest = self.samples.back().map(|s| s.at);
        self.samples.iter().filter_map(move |s| {
            let v = s.values[series.index()]?;
            Some((newest?.saturating_duration_since(s.at), v))
        })
    }

    /// The most recent value of `series`.
    pub fn latest(&self, series: Series) -> Option<f64> {
        self.points(series).last().map(|(_, v)| v)
    }

    /// The last pushed metrics, for raw used and total figures.
    pub fn last(&self) -> Option<&Metrics> {
        self.last.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn m(rx: Option<u64>) -> Metrics {
        Metrics {
            cpu_percent: Some(10.0),
            memory_used: Some(50),
            memory_total: Some(200),
            disk_used: None,
            disk_total: Some(0),
            net_received: rx,
            net_sent: rx,
            typesense_memory_active: None,
            raw: BTreeMap::new(),
        }
    }

    const S5: Duration = Duration::from_secs(5);

    #[test]
    fn drops_points_older_than_window() {
        let t0 = Instant::now();
        let mut h = MetricsHistory::new(Duration::from_secs(12));
        for i in 0..4 {
            h.push(t0 + S5 * i, &m(None));
        }
        let ages: Vec<_> = h.points(Series::Cpu).map(|(a, _)| a).collect();
        assert_eq!(ages, vec![S5 * 2, S5, Duration::ZERO]);
    }

    #[test]
    fn network_rate_from_counters() {
        let t0 = Instant::now();
        let mut h = MetricsHistory::new(Duration::from_secs(60));
        h.push(t0, &m(Some(1000)));
        h.push(t0 + S5, &m(Some(6000)));
        assert_eq!(h.latest(Series::NetRx), Some(1000.0));
        assert_eq!(h.latest(Series::NetTx), Some(1000.0));
    }

    #[test]
    fn counter_reset_skips_rate() {
        let t0 = Instant::now();
        let mut h = MetricsHistory::new(Duration::from_secs(60));
        h.push(t0, &m(Some(6000)));
        h.push(t0 + S5, &m(Some(100)));
        assert_eq!(h.points(Series::NetRx).count(), 0);
        h.push(t0 + S5 * 2, &m(Some(600)));
        assert_eq!(h.latest(Series::NetRx), Some(100.0));
    }

    #[test]
    fn zero_elapsed_skips_rate() {
        let t0 = Instant::now();
        let mut h = MetricsHistory::new(Duration::from_secs(60));
        h.push(t0, &m(Some(1)));
        h.push(t0, &m(Some(2)));
        assert_eq!(h.points(Series::NetRx).count(), 0);
    }

    #[test]
    fn missing_fields_skip_series() {
        let t0 = Instant::now();
        let mut h = MetricsHistory::new(Duration::from_secs(60));
        h.push(t0, &m(None));
        h.push(t0 + S5, &m(None));
        assert_eq!(h.points(Series::NetRx).count(), 0);
        assert_eq!(h.points(Series::Disk).count(), 0); // total 0 and used missing
        assert_eq!(h.latest(Series::Memory), Some(25.0));
        assert_eq!(h.last().and_then(|l| l.memory_total), Some(200));
        assert_eq!(h.revision(), 2);
    }
}
