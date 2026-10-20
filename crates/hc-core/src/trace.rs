//! The trace bus: `tracing` layer → bounded ring buffer → broadcast channel.
//!
//! Socket and session events (frames, decrypted JSON, handshake steps, reconnects, errors) are
//! emitted here, and the frame inspector is fed from the broadcast side.
//!
//! Both halves are bounded: the ring buffer drops its oldest event once full, and a subscriber
//! that cannot keep up is lapped (`RecvError::Lagged`). [`TraceBus::emitted`] reports the true
//! count, so drops are visible.
//!
//! Secrets are carried in [`hc_crypto::Secret`](../../hc_crypto/struct.Secret.html), whose
//! `Debug` redacts; this layer captures fields through `Debug`, so a wrapped secret cannot reach
//! the ring buffer or an SSE frame.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Severity of a trace event, mirroring `tracing::Level` in a serialisable form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceLevel {
    /// Something failed and the caller must react.
    Error,
    /// Something unexpected that the system recovered from.
    Warn,
    /// Normal lifecycle milestones.
    Info,
    /// Detail useful while diagnosing a session.
    Debug,
    /// Per-frame firehose.
    Trace,
}

impl From<Level> for TraceLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::ERROR => Self::Error,
            Level::WARN => Self::Warn,
            Level::INFO => Self::Info,
            Level::DEBUG => Self::Debug,
            _ => Self::Trace,
        }
    }
}

/// An event on its way into the bus, before the bus assigns it a sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRecord {
    level: TraceLevel,
    target: String,
    message: String,
    fields: BTreeMap<String, String>,
}

impl TraceRecord {
    /// Builds a record from the three fields every event has.
    #[must_use]
    pub fn new(level: TraceLevel, target: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level,
            target: target.into(),
            message: message.into(),
            fields: BTreeMap::new(),
        }
    }

    /// Attaches a structured field.
    ///
    /// Values are rendered to strings at the edge, so `Secret<T>`'s redacting `Debug` applies
    /// before anything is stored.
    #[must_use]
    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

/// An event as it leaves the bus: a [`TraceRecord`] plus its bus-assigned sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Monotonic, gap-free, assigned on emission. A gap between two events a subscriber sees
    /// therefore means "dropped", and the inspector can say so.
    pub seq: u64,
    /// When the bus received the event, in milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    /// Severity.
    pub level: TraceLevel,
    /// The `tracing` target the event came from.
    pub target: String,
    /// The event's message field.
    pub message: String,
    /// Remaining structured fields, rendered.
    pub fields: BTreeMap<String, String>,
}

/// The bounded ring buffer plus the broadcast fan-out.
///
/// Cheap to share: wrap it in an [`Arc`] and hand clones to every device task.
#[derive(Debug)]
pub struct TraceBus {
    capacity: usize,
    ring: Mutex<VecDeque<TraceEvent>>,
    emitted: AtomicU64,
    sender: broadcast::Sender<TraceEvent>,
}

impl TraceBus {
    /// Creates a bus retaining at most `capacity` events.
    ///
    /// A `capacity` of zero is raised to one, because a zero-capacity broadcast channel is not
    /// constructible.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _receiver) = broadcast::channel(capacity);
        Self {
            capacity,
            ring: Mutex::new(VecDeque::with_capacity(capacity)),
            emitted: AtomicU64::new(0),
            sender,
        }
    }

    /// The ring buffer's ceiling.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many events have ever been emitted, including those since dropped.
    #[must_use]
    pub fn emitted(&self) -> u64 {
        self.emitted.load(Ordering::Relaxed)
    }

    /// Subscribes to the live fan-out.
    ///
    /// The bus is live, not a replay log: a subscriber sees events emitted after it subscribed.
    /// Use [`recent`](Self::recent) for the backlog.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<TraceEvent> {
        self.sender.subscribe()
    }

    /// Publishes an event to the ring buffer and every subscriber. Returns its sequence number.
    ///
    /// Never blocks on a slow subscriber and never fails: with no subscribers, the broadcast
    /// send is a no-op, which is the normal state when nobody has the inspector open.
    pub fn emit(&self, record: TraceRecord) -> u64 {
        let seq = self.emitted.fetch_add(1, Ordering::Relaxed);
        let event = TraceEvent {
            seq,
            timestamp_ms: now_ms(),
            level: record.level,
            target: record.target,
            message: record.message,
            fields: record.fields,
        };

        {
            let mut ring = self.ring.lock();
            if ring.len() >= self.capacity {
                ring.pop_front();
            }
            ring.push_back(event.clone());
        }

        // `Err` means "no subscribers", which is not an error condition here.
        let _ = self.sender.send(event);

        seq
    }

    /// The retained backlog, oldest first.
    #[must_use]
    pub fn recent(&self) -> Vec<TraceEvent> {
        self.ring.lock().iter().cloned().collect()
    }

    /// Drops the retained backlog. The sequence counter is deliberately not reset.
    pub fn clear(&self) {
        self.ring.lock().clear();
    }
}

/// A `tracing_subscriber` layer that fans every event into a [`TraceBus`].
///
/// Register it alongside the usual `fmt` layer; the bus is an additional sink, not a
/// replacement for stderr logging.
#[derive(Debug, Clone)]
pub struct TraceBusLayer {
    bus: Arc<TraceBus>,
}

impl TraceBusLayer {
    /// Wraps a shared bus as a `tracing` layer.
    #[must_use]
    pub const fn new(bus: Arc<TraceBus>) -> Self {
        Self { bus }
    }

    /// The bus this layer writes to.
    #[must_use]
    pub const fn bus(&self) -> &Arc<TraceBus> {
        &self.bus
    }
}

impl<S: Subscriber> Layer<S> for TraceBusLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let metadata = event.metadata();
        self.bus.emit(TraceRecord {
            level: TraceLevel::from(*metadata.level()),
            target: metadata.target().to_owned(),
            message: visitor.message,
            fields: visitor.fields,
        });
    }
}

/// Renders `tracing` fields to strings, splitting `message` out from the rest.
///
/// Everything goes through `Debug`, which is exactly why `Secret<T>` redacts there.
#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl FieldVisitor {
    fn store(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields.insert(field.name().to_owned(), rendered);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.store(field, format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.store(field, value.to_owned());
    }
}

/// The wall clock in milliseconds since the Unix epoch; zero if the clock reads before it.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
