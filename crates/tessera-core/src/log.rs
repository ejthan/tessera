// SPDX-License-Identifier: GPL-3.0-or-later

//! The in-memory request log: two ring buffers, one for user actions, one for polling.

use std::collections::VecDeque;
use std::sync::Arc;

use tessera_client::{Method, Origin, RequestEvent};
use uuid::Uuid;

/// Capacity of the buffer for [`Origin::User`] entries.
pub const USER_CAP: usize = 5000;
/// Capacity of the buffer for [`Origin::Polling`] entries.
pub const POLLING_CAP: usize = 500;

/// One logged request with the connection it belongs to.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Id of the connection profile.
    pub connection: Uuid,
    /// Display name of the connection.
    pub connection_name: Arc<str>,
    /// What the client reported.
    pub event: RequestEvent,
}

impl LogEntry {
    /// A request counts as failed when its status is 400 or above, or absent.
    pub fn is_error(&self) -> bool {
        self.event.status.is_none_or(|s| s >= 400)
    }
}

/// Which entries [`RequestLog::entries`] returns. The default shows everything except polling.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only failed requests. Overrides `show_polling`: failed polling requests are included.
    pub errors_only: bool,
    /// Only POST, PUT, PATCH and DELETE.
    pub writes_only: bool,
    /// Only these methods; empty means all.
    pub methods: Vec<Method>,
    /// Only these connections; empty means all.
    pub connections: Vec<Uuid>,
    /// Include polling entries.
    pub show_polling: bool,
}

impl Filter {
    fn matches(&self, e: &LogEntry) -> bool {
        let m = &e.event.method;
        (self.show_polling || self.errors_only || e.event.origin == Origin::User)
            && (!self.errors_only || e.is_error())
            && (!self.writes_only || ![Method::GET, Method::HEAD, Method::OPTIONS].contains(m))
            && (self.methods.is_empty() || self.methods.contains(m))
            && (self.connections.is_empty() || self.connections.contains(&e.connection))
    }
}

/// Bounded log of requests; the oldest entry of a full buffer is dropped first.
#[derive(Debug)]
pub struct RequestLog {
    user: VecDeque<LogEntry>,
    polling: VecDeque<LogEntry>,
    user_cap: usize,
    polling_cap: usize,
}

impl Default for RequestLog {
    fn default() -> Self {
        Self::new(USER_CAP, POLLING_CAP)
    }
}

impl RequestLog {
    /// A log holding at most `user_cap` user and `polling_cap` polling entries.
    pub fn new(user_cap: usize, polling_cap: usize) -> Self {
        Self {
            user: VecDeque::new(),
            polling: VecDeque::new(),
            user_cap,
            polling_cap,
        }
    }

    /// Adds an entry to the buffer of its origin, evicting that buffer's oldest if full.
    pub fn push(&mut self, entry: LogEntry) {
        let (buf, cap) = match entry.event.origin {
            Origin::User => (&mut self.user, self.user_cap),
            Origin::Polling => (&mut self.polling, self.polling_cap),
        };
        if buf.len() >= cap {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Matching entries from both buffers, oldest first (ordered by event id).
    pub fn entries(&self, filter: &Filter) -> Vec<&LogEntry> {
        let mut out: Vec<_> = self
            .user
            .iter()
            .chain(&self.polling)
            .filter(|e| filter.matches(e))
            .collect();
        out.sort_by_key(|e| e.event.id);
        out
    }

    /// The entry with this event id, if it is still in the log.
    pub fn get(&self, id: u64) -> Option<&LogEntry> {
        self.user
            .iter()
            .chain(&self.polling)
            .find(|e| e.event.id == id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    pub(crate) fn entry(id: u64, method: Method, origin: Origin, status: Option<u16>) -> LogEntry {
        LogEntry {
            connection: Uuid::nil(),
            connection_name: "dev".into(),
            event: RequestEvent {
                id,
                started_at: SystemTime::UNIX_EPOCH,
                node: "http://localhost:8108".parse().expect("url"),
                method,
                path: "/health".into(),
                query: None,
                request_headers: vec![],
                key_prefix: String::new(),
                request_body: None,
                response_body: None,
                status,
                duration: Duration::ZERO,
                size: None,
                origin,
                error: None,
            },
        }
    }

    fn ids(log: &RequestLog, f: &Filter) -> Vec<u64> {
        log.entries(f).iter().map(|e| e.event.id).collect()
    }

    #[test]
    fn polling_does_not_evict_user_entries() {
        let mut log = RequestLog::default();
        log.push(entry(0, Method::GET, Origin::User, Some(200)));
        for i in 1..=600 {
            log.push(entry(i, Method::GET, Origin::Polling, Some(200)));
        }
        let all = ids(
            &log,
            &Filter {
                show_polling: true,
                ..Filter::default()
            },
        );
        assert_eq!(all.len(), 501);
        assert_eq!(all[0], 0);
        assert_eq!(all[1], 101);
    }

    #[test]
    fn user_cap_evicts_oldest() {
        let mut log = RequestLog::new(3, 3);
        for i in 1..=5 {
            log.push(entry(i, Method::GET, Origin::User, Some(200)));
        }
        assert_eq!(ids(&log, &Filter::default()), [3, 4, 5]);
        assert!(log.get(2).is_none());
        assert!(log.get(3).is_some());
    }

    #[test]
    fn default_filter_hides_polling_but_errors_show_failed_polling() {
        let mut log = RequestLog::default();
        log.push(entry(1, Method::GET, Origin::User, Some(200)));
        log.push(entry(2, Method::GET, Origin::Polling, None));
        log.push(entry(3, Method::GET, Origin::Polling, Some(200)));
        assert_eq!(ids(&log, &Filter::default()), [1]);
        let errors = Filter {
            errors_only: true,
            ..Filter::default()
        };
        assert_eq!(ids(&log, &errors), [2]);
    }

    #[test]
    fn writes_only_filters_methods() {
        let mut log = RequestLog::default();
        log.push(entry(1, Method::GET, Origin::User, Some(200)));
        log.push(entry(2, Method::POST, Origin::User, Some(201)));
        log.push(entry(3, Method::DELETE, Origin::User, Some(500)));
        let f = Filter {
            writes_only: true,
            ..Filter::default()
        };
        assert_eq!(ids(&log, &f), [2, 3]);
        let f = Filter {
            methods: vec![Method::DELETE],
            ..Filter::default()
        };
        assert_eq!(ids(&log, &f), [3]);
    }

    #[test]
    fn merge_orders_by_id() {
        let mut log = RequestLog::default();
        log.push(entry(1, Method::GET, Origin::User, Some(200)));
        log.push(entry(2, Method::GET, Origin::Polling, Some(200)));
        log.push(entry(3, Method::GET, Origin::User, Some(200)));
        let f = Filter {
            show_polling: true,
            ..Filter::default()
        };
        assert_eq!(ids(&log, &f), [1, 2, 3]);
    }
}
