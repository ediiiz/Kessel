//! The HTTP end of the write path.
//!
//! Every write goes through `hc_core::write::GuardedWriter`, the only write path. The guard itself
//! refuses nothing; the one thing that does is `hc_core::write::check_access`, the feature's
//! declared `access`, applied when the mapping decides whether a feature gets a `…/set` topic. This
//! module resolves a slug to that device's writer.
//!
//! There is one long-lived writer per connected device rather than one per request, so every write
//! on a connection spends `msgID`s from that session's own allocator. A device with no live
//! connection has no entry, and a write to it fails: writes are never queued for a disconnected
//! appliance.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use hc_core::bridge::BoxedTransport;
use hc_core::write::{GuardedWriter, WriteCommand, WriteTransport};
use parking_lot::Mutex;

/// The long-lived, type-erased writer for one device's connection.
///
/// Type-erased because the AES and TLS-PSK session writers are different types. The erasure
/// happens inside `hc-core`, under the guard, in [`hc_core::bridge::take_guarded_session_boxed`].
pub type LiveWriter = GuardedWriter<BoxedTransport>;

/// A shared handle on one device's writer.
///
/// An async mutex because the lock is held across the `await` that reaches the socket. Concurrent
/// writes to one device serialise here, which keeps the connection's `msgID` sequence ordered.
pub type SharedWriter = Arc<tokio::sync::Mutex<LiveWriter>>;

/// The daemon's per-device writers: one per live connection, installed when a device reaches
/// CONNECTED and removed when its connection ends.
///
/// Every connected device gets an entry, the hob and the oven included; which features are writable
/// is decided by each feature's declared access, not by appliance type.
#[derive(Debug, Default)]
pub struct WriterRegistry {
    /// Keyed by device slug, never by an appliance identifier.
    writers: Mutex<BTreeMap<String, SharedWriter>>,
}

impl WriterRegistry {
    /// An empty registry — the state before any device connects.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs `writer` as `slug`'s writer, replacing any previous one.
    ///
    /// A reconnect brings a new session and `msgID` allocator, so the old writer must go. A request
    /// still holding the old `Arc` fails on the closed socket and cannot disturb the new sequence.
    pub fn install(&self, slug: impl Into<String>, writer: LiveWriter) {
        let shared = Arc::new(tokio::sync::Mutex::new(writer));
        self.writers.lock().insert(slug.into(), shared);
    }

    /// Removes `slug`'s writer — called when its connection ends, so writes fail closed again.
    pub fn remove(&self, slug: &str) {
        self.writers.lock().remove(slug);
    }

    /// `slug`'s writer, if the device has a live connection.
    ///
    /// The `Arc` is cloned out and the registry lock released before the caller takes the writer's
    /// own lock, so one device's slow write cannot block lookups for every other device.
    #[must_use]
    pub fn writer(&self, slug: &str) -> Option<SharedWriter> {
        self.writers.lock().get(slug).map(Arc::clone)
    }

    /// How many devices currently have a live writer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.writers.lock().len()
    }

    /// Whether no device has a live writer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.writers.lock().is_empty()
    }
}

/// The transport a write gets when its device has no live session: it refuses, with the reason.
///
/// The caller still routes it through a `GuardedWriter`, so the write takes the same single path as
/// every other write.
#[must_use]
pub fn no_live_session_transport() -> BoxedTransport {
    Box::new(RefusingTransport)
}

/// The transport [`no_live_session_transport`] hands out. Refuses, with the reason.
#[derive(Debug, Clone, Copy)]
struct RefusingTransport;

#[async_trait]
impl WriteTransport for RefusingTransport {
    async fn transmit(&mut self, _command: &WriteCommand) -> Result<(), String> {
        Err(
            "this device has no live session, so there is nothing to write to; no bytes were sent. \
             A write is not queued for a disconnected appliance"
                .to_owned(),
        )
    }
}
