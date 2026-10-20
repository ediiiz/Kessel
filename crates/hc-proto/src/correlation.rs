//! Request/response correlation and the per-request timeout.
//!
//! Each outstanding `GET` registers its `msgID`; an inbound `RESPONSE` is matched by `msgID`, not
//! by arrival order, because several requests are outstanding at once during the handshake. The
//! 20 s timeout is driven by an injected [`Clock`] so it is testable without a real timer.

use std::collections::HashMap;

/// The per-request correlation timeout, in milliseconds (20 s).
pub const RESPONSE_TIMEOUT_MS: u64 = 20_000;

/// A monotonic clock, in milliseconds, injected so timeouts are deterministic in tests.
pub trait Clock {
    /// The current time, in milliseconds, on some monotonic scale.
    fn now_ms(&self) -> u64;
}

#[derive(Debug, Clone)]
struct Pending {
    resource: String,
    deadline_ms: u64,
}

/// The table of outstanding requests, keyed by `msgID`.
#[derive(Debug, Default)]
pub struct Correlator {
    pending: HashMap<u64, Pending>,
}

impl Correlator {
    /// An empty correlator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an outstanding request; it times out [`RESPONSE_TIMEOUT_MS`] after `now_ms`.
    pub fn register(&mut self, msg_id: u64, resource: &str, now_ms: u64) {
        self.pending.insert(
            msg_id,
            Pending {
                resource: resource.to_owned(),
                deadline_ms: now_ms.saturating_add(RESPONSE_TIMEOUT_MS),
            },
        );
    }

    /// Resolves a response by `msgID`, removing it and returning the resource it answers. `None`
    /// for an unknown `msgID`: an uncorrelated RESPONSE, which is not fatal.
    pub fn resolve(&mut self, msg_id: u64) -> Option<String> {
        self.pending.remove(&msg_id).map(|pending| pending.resource)
    }

    /// Whether a `msgID` is currently outstanding.
    #[must_use]
    pub fn is_pending(&self, msg_id: u64) -> bool {
        self.pending.contains_key(&msg_id)
    }

    /// The `msgID`s whose deadline has passed as of `now_ms`.
    #[must_use]
    pub fn timed_out(&self, now_ms: u64) -> Vec<u64> {
        self.pending
            .iter()
            .filter(|(_, pending)| now_ms >= pending.deadline_ms)
            .map(|(msg_id, _)| *msg_id)
            .collect()
    }

    /// Number of outstanding requests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether there are no outstanding requests.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
