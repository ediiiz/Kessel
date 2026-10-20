//! `tracing` wiring: the stderr `fmt` layer and the trace-bus layer behind the frame inspector,
//! each with its own filter.
//!
//! The filters are per layer so the inspector does not depend on log verbosity: `KESSEL_LOG`
//! governs stderr only, and the bus takes everything down to `TRACE` unless `KESSEL_INSPECTOR_LOG`
//! says otherwise. The bus is bounded, so that costs a fixed amount of memory.

use std::sync::Arc;

use hc_core::trace::{TraceBus, TraceBusLayer};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt as _;

/// How many trace events the ring buffer retains for a newly-opened frame inspector.
pub const TRACE_BUFFER_CAPACITY: usize = 2048;

/// Controls stderr log verbosity.
pub const LOG_ENV: &str = "KESSEL_LOG";

/// Controls what reaches the frame inspector. Independent of [`LOG_ENV`] by design.
pub const INSPECTOR_ENV: &str = "KESSEL_INSPECTOR_LOG";

/// Builds the composed subscriber and the bus it feeds.
///
/// Returned rather than installed so tests can use `tracing::subscriber::set_default` instead of a
/// process-global `init`.
#[must_use]
pub fn subscriber(bus: &Arc<TraceBus>) -> impl tracing::Subscriber + Send + Sync + use<> {
    let stderr_filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    // The inspector's own filter, `trace` by default, independent of `KESSEL_LOG`.
    let inspector_filter =
        EnvFilter::try_from_env(INSPECTOR_ENV).unwrap_or_else(|_| EnvFilter::new("trace"));
    subscriber_with_filters(bus, stderr_filter, inspector_filter)
}

/// [`subscriber`] with both filters supplied explicitly.
///
/// The environment-free form, so tests can set filters without mutating the process environment
/// (which is `unsafe` in edition 2024).
#[must_use]
pub fn subscriber_with_filters(
    bus: &Arc<TraceBus>,
    stderr_filter: EnvFilter,
    inspector_filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync + use<> {
    Registry::default()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(stderr_filter),
        )
        .with(TraceBusLayer::new(Arc::clone(bus)).with_filter(inspector_filter))
}

/// Creates the bus, installs the subscriber process-wide, and hands the bus back.
///
/// # Panics
///
/// If a global subscriber is already installed — a programming error, not a runtime condition.
#[must_use]
pub fn install() -> Arc<TraceBus> {
    let bus = Arc::new(TraceBus::new(TRACE_BUFFER_CAPACITY));
    subscriber(&bus).init();
    bus
}
