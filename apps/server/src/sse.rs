//! The SSE stream that feeds the frame inspector.
//!
//! There is one multiplexed route, to stay under the HTTP/1.1 per-origin connection cap; a frame's
//! kind travels in the SSE `event:` field ([`crate::types::SseEventKind`]).
//!
//! `hc_core::trace::TraceBus` is live, not a replay log, so the stream subscribes first, replays
//! the retained ring buffer, then drops live frames the backlog already carried: no gap, no
//! duplicate.
//!
//! `X-Accel-Buffering: no` and `Cache-Control: no-cache` are set because a buffering proxy turns a
//! live stream into one that delivers nothing until it closes.
//!
//! A browser that cannot keep up is lapped by the broadcast channel and the lapped events are
//! skipped silently: `seq` is gap-free, so a gap in it is the report, and emitting a `tracing`
//! event from here would feed the bus from its own reader.

use std::collections::BTreeSet;
use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::{Stream, StreamExt as _, stream};
use hc_core::trace::TraceBus;
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;

use crate::api::ApiFailure;
use crate::state::AppState;
use crate::types::{TraceFrame, TraceLevel, TraceSource};

/// The header that tells a reverse proxy not to buffer this response.
pub const NO_BUFFERING_HEADER: &str = "x-accel-buffering";

/// Its value.
pub const NO_BUFFERING_VALUE: &str = "no";

/// How often a comment is sent to keep an idle stream (and the proxies in front of it) alive.
const KEEP_ALIVE_SECS: u64 = 15;

/// The retained backlog followed by the live feed, as inspector frames.
pub fn frames(bus: &TraceBus) -> impl Stream<Item = TraceFrame> + Send + 'static {
    // Subscribe BEFORE snapshotting, so nothing emitted between the two is lost.
    let live = BroadcastStream::new(bus.subscribe());
    let backlog = bus.recent();
    // Everything up to and including this sequence number is already in the backlog.
    let replayed_through = backlog.last().map(|event| event.seq);

    // Marked as replayed so a subscriber that acts on connection transitions does not act on the
    // whole history at connect.
    let backlog = stream::iter(backlog.into_iter().map(TraceFrame::from_retained));
    let live = live.filter_map(move |received| {
        // `Err` is `Lagged`: the subscriber was lapped. Skipped silently — see the module docs.
        let frame = received.ok().map(TraceFrame::from_event);
        std::future::ready(frame.filter(|frame| replayed_through.is_none_or(|seq| frame.seq > seq)))
    });

    backlog.chain(live)
}

/// The two knobs `GET /api/events` accepts, as they arrive in the query string.
///
/// Both are optional and default to "everything".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventsQuery {
    /// `level=warn` — the least severe level to send, i.e. "warn and worse".
    pub level: Option<String>,
    /// `source=mqtt,homeconnect` — a comma-separated list of [`TraceSource`] names.
    pub source: Option<String>,
}

/// A parsed [`EventsQuery`]: what the stream is allowed to carry.
#[derive(Debug, Clone)]
pub struct EventFilter {
    /// The verbosity ceiling. [`TraceLevel::Trace`] admits everything.
    pub level: TraceLevel,
    /// The groups to include; empty means "no restriction", not "nothing".
    pub sources: BTreeSet<TraceSource>,
}

impl Default for EventFilter {
    fn default() -> Self {
        Self {
            level: TraceLevel::Trace,
            sources: BTreeSet::new(),
        }
    }
}

impl EventFilter {
    /// Whether a frame survives.
    #[must_use]
    pub fn admits(&self, frame: &TraceFrame) -> bool {
        frame.level.admits(self.level)
            && (self.sources.is_empty() || self.sources.contains(&frame.source))
    }
}

impl TryFrom<EventsQuery> for EventFilter {
    type Error = ApiFailure;

    /// Rejects an unknown name rather than ignoring it, so the operator is never shown a stream
    /// other than the one they asked for.
    fn try_from(query: EventsQuery) -> Result<Self, Self::Error> {
        let level = match query.level.as_deref().map(str::trim) {
            None | Some("") => TraceLevel::Trace,
            Some(name) => TraceLevel::parse(name).ok_or_else(|| {
                ApiFailure::bad_request(format!(
                    "unknown log level `{name}`; expected one of {}",
                    names(TraceLevel::ALL.iter().map(|level| level.as_str()))
                ))
            })?,
        };

        let mut sources = BTreeSet::new();
        for name in query
            .source
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let source = TraceSource::parse(name).ok_or_else(|| {
                ApiFailure::bad_request(format!(
                    "unknown log source `{name}`; expected one of {}",
                    names(TraceSource::ALL.iter().map(|source| source.as_str()))
                ))
            })?;
            sources.insert(source);
        }

        Ok(Self { level, sources })
    }
}

/// `a, b, c` — the accepted values, for an error message.
fn names<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values.collect::<Vec<_>>().join(", ")
}

/// `GET /api/events` — the multiplexed stream, filtered per [`EventsQuery`].
///
/// The bus takes everything down to `TRACE`, so filtering happens here rather than in the browser
/// to keep the wire quiet. `seq` is not renumbered: a filtered stream has holes in it by
/// construction, and renumbering would hide a genuine drop.
pub async fn events(State(state): State<AppState>, Query(query): Query<EventsQuery>) -> Response {
    let filter = match EventFilter::try_from(query) {
        Ok(filter) => filter,
        Err(failure) => return failure.into_response(),
    };
    let stream = frames(state.bus())
        .filter(move |frame| std::future::ready(filter.admits(frame)))
        .map(|frame| Ok::<Event, Infallible>(event_for(&frame)));
    let sse = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(KEEP_ALIVE_SECS)));
    (
        [
            (NO_BUFFERING_HEADER, NO_BUFFERING_VALUE),
            ("cache-control", "no-cache"),
        ],
        sse,
    )
        .into_response()
}

/// Renders one frame as an SSE event.
///
/// `event:` is the frame's kind, which a browser's `EventSource` dispatches on. `data:` is the
/// frame as JSON and `id:` its sequence number, so a reconnecting client can send `Last-Event-ID`.
fn event_for(frame: &TraceFrame) -> Event {
    let kind = frame.kind.as_str();
    // `json_data` only fails if the value cannot serialise; `TraceFrame` is a plain struct of
    // strings, so the fallback is unreachable in practice and carries no frame content.
    Event::default()
        .event(kind)
        .id(frame.seq.to_string())
        .json_data(frame)
        .unwrap_or_else(|_| Event::default().event(kind).data("{}"))
}
