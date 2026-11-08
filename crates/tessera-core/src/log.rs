// SPDX-License-Identifier: GPL-3.0-or-later

//! The in-memory request log: two ring buffers, one for user actions, one for polling.

use std::collections::VecDeque;
use std::sync::Arc;

use tessera_client::{Body, Method, Origin, RequestEvent};
use uuid::Uuid;

/// Capacity of the buffer for [`Origin::User`] entries.
pub const USER_CAP: usize = 5000;
/// Capacity of the buffer for [`Origin::Polling`] entries.
pub const POLLING_CAP: usize = 500;
/// Bytes of full bodies kept, over all entries (64 MiB).
pub const FULL_BODY_BUDGET: usize = 64 << 20;

/// Which body of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyPart {
    /// What was sent.
    Request,
    /// What came back.
    Response,
}

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
#[derive(Debug, Clone, Default, PartialEq)]
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
            && (!self.writes_only
                || (![Method::GET, Method::HEAD, Method::OPTIONS].contains(m)
                    // A search sent as POST reads; a DELETE or PUT on a resource of that
                    // name still writes.
                    && !(*m == Method::POST && e.event.path.ends_with("/multi_search"))))
            && (self.methods.is_empty() || self.methods.contains(m))
            && (self.connections.is_empty() || self.connections.contains(&e.connection))
    }
}

/// Bounded log of requests; the oldest entry of a full buffer is dropped first. Full bodies
/// live apart under [`FULL_BODY_BUDGET`], so that budget alone bounds their memory.
pub struct RequestLog {
    user: VecDeque<LogEntry>,
    polling: VecDeque<LogEntry>,
    user_cap: usize,
    polling_cap: usize,
    full: VecDeque<(u64, BodyPart, Arc<str>)>,
    full_bytes: usize,
}

// Lengths only: a full body can hold a key.
impl std::fmt::Debug for RequestLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestLog")
            .field("user", &self.user.len())
            .field("polling", &self.polling.len())
            .field("full_bytes", &self.full_bytes)
            .finish_non_exhaustive()
    }
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
            full: VecDeque::new(),
            full_bytes: 0,
        }
    }

    /// Moves the full body of `body` out of the entry, evicting the oldest past the budget.
    fn keep_full(&mut self, id: u64, part: BodyPart, body: Option<&mut Body>) {
        let Some(full) = body.and_then(|b| b.full.take()) else {
            return;
        };
        self.full_bytes += full.len();
        self.full.push_back((id, part, full));
        while self.full_bytes > FULL_BODY_BUDGET {
            let Some((.., oldest)) = self.full.pop_front() else {
                break;
            };
            self.full_bytes -= oldest.len();
        }
    }

    /// Adds an entry to the buffer of its origin, evicting that buffer's oldest if full.
    pub fn push(&mut self, mut entry: LogEntry) {
        let id = entry.event.id;
        self.keep_full(id, BodyPart::Request, entry.event.request_body.as_mut());
        self.keep_full(id, BodyPart::Response, entry.event.response_body.as_mut());
        let (buf, cap) = match entry.event.origin {
            Origin::User => (&mut self.user, self.user_cap),
            Origin::Polling => (&mut self.polling, self.polling_cap),
        };
        buf.push_back(entry);
        while buf.len() > cap {
            let Some(dropped) = buf.pop_front() else {
                break;
            };
            let id = dropped.event.id;
            let (full, bytes) = (&mut self.full, &mut self.full_bytes);
            full.retain(|(owner, _, text)| {
                let keep = *owner != id;
                if !keep {
                    *bytes -= text.len();
                }
                keep
            });
        }
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

    /// Number of entries in both buffers.
    pub fn len(&self) -> usize {
        self.user.len() + self.polling.len()
    }

    /// Whether the log holds no entry.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The highest event id in the log. With [`Self::len`] it tells whether the log changed.
    pub fn last_id(&self) -> Option<u64> {
        let last = |buf: &VecDeque<LogEntry>| buf.back().map(|e| e.event.id);
        last(&self.user).max(last(&self.polling))
    }

    /// The entry with this event id, if it is still in the log.
    pub fn get(&self, id: u64) -> Option<&LogEntry> {
        self.user
            .iter()
            .chain(&self.polling)
            .find(|e| e.event.id == id)
    }

    /// The whole text of a cut body, while the budget still holds it.
    pub fn full_body(&self, id: u64, part: BodyPart) -> Option<Arc<str>> {
        let kept = self.full.iter().find(|(i, p, _)| *i == id && *p == part);
        kept.map(|(.., full)| full.clone())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

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
        // Eviction keeps the length, so the last id tells that the log changed.
        assert_eq!((log.len(), log.last_id()), (3, Some(5)));
        log.push(entry(6, Method::GET, Origin::Polling, Some(200)));
        assert_eq!((log.len(), log.last_id()), (4, Some(6)));
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
    fn zero_cap_keeps_nothing() {
        let mut log = RequestLog::new(0, 0);
        log.push(entry(1, Method::GET, Origin::User, Some(200)));
        log.push(entry(2, Method::GET, Origin::Polling, Some(200)));
        assert!(log.get(1).is_none() && log.get(2).is_none());
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
    fn writes_filter_skips_multi_search() {
        let mut log = RequestLog::default();
        let mut search = entry(1, Method::POST, Origin::User, Some(200));
        search.event.path = "/multi_search".into();
        log.push(search);
        log.push(entry(2, Method::POST, Origin::User, Some(201)));
        let f = Filter {
            writes_only: true,
            ..Filter::default()
        };
        assert_eq!(ids(&log, &f), [2]);
        // The same name on a resource is still a write.
        let mut log = RequestLog::default();
        for (id, method, path) in [
            (1, Method::DELETE, "/collections/multi_search"),
            (2, Method::DELETE, "/aliases/multi_search"),
            (3, Method::PUT, "/presets/multi_search"),
        ] {
            let mut e = entry(id, method, Origin::User, Some(200));
            e.event.path = path.into();
            log.push(e);
        }
        assert_eq!(ids(&log, &f), [1, 2, 3]);
    }

    fn with_full(id: u64, response: &Arc<str>) -> LogEntry {
        let mut e = entry(id, Method::GET, Origin::User, Some(200));
        let body = |full: &Arc<str>| Body {
            text: "cut".into(),
            original_len: full.len(),
            full: Some(full.clone()),
        };
        e.event.response_body = Some(body(response));
        e
    }

    #[test]
    fn push_moves_full_bodies_out_of_the_entry() {
        let mut log = RequestLog::default();
        let full: Arc<str> = "whole".into();
        let mut e = with_full(1, &full);
        e.event.request_body = Some(Body {
            text: "cut".into(),
            original_len: 7,
            full: Some("request".into()),
        });
        log.push(e);
        let kept = log.get(1).unwrap();
        assert!(kept.event.response_body.as_ref().unwrap().full.is_none());
        assert!(kept.event.request_body.as_ref().unwrap().full.is_none());
        assert_eq!(log.full_body(1, BodyPart::Response), Some(full));
        assert_eq!(log.full_body(1, BodyPart::Request), Some("request".into()));
        assert_eq!(log.full_body(2, BodyPart::Response), None);
    }

    #[test]
    fn budget_evicts_the_oldest() {
        let mut log = RequestLog::default();
        let big: Arc<str> = "x".repeat(FULL_BODY_BUDGET / 2).into();
        for id in 1..=3 {
            log.push(with_full(id, &big));
        }
        assert!(log.full_body(1, BodyPart::Response).is_none());
        assert!(log.full_body(2, BodyPart::Response).is_some());
        assert!(log.full_body(3, BodyPart::Response).is_some());
        assert_eq!(log.full_bytes, FULL_BODY_BUDGET);
    }

    #[test]
    fn full_body_of_an_evicted_entry_is_none() {
        let mut log = RequestLog::default();
        log.push(with_full(1, &"short".into()));
        log.push(with_full(2, &"x".repeat(FULL_BODY_BUDGET + 1).into()));
        assert!(log.full_body(1, BodyPart::Response).is_none());
        assert!(
            log.full_body(2, BodyPart::Response).is_none(),
            "over budget alone"
        );
        assert!(log.get(1).is_some(), "the entry itself stays");
    }

    #[test]
    fn the_ring_releases_the_full_bodies_of_what_it_drops() {
        let mut log = RequestLog::new(1, 1);
        log.push(with_full(1, &"first".into()));
        log.push(with_full(2, &"second".into()));
        assert!(log.full_body(1, BodyPart::Response).is_none());
        assert!(log.full_body(2, BodyPart::Response).is_some());
        assert_eq!(log.full_bytes, "second".len());
    }

    #[test]
    fn debug_prints_no_full_body() {
        let mut log = RequestLog::default();
        log.push(with_full(1, &"sk-secret-tail".into()));
        assert!(!format!("{log:?}").contains("sk-secret-tail"));
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
