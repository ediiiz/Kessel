//! Write acknowledgements: pairing the appliance's RESPONSE with the write that caused it.
//!
//! The appliance answers every write with a `RESPONSE` carrying the write's `msgID`: no `code` when
//! it took the value, an error `code` when it refused. The writer registers its `msgID` before
//! sending and waits; the connection's reader resolves it when the response arrives.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hc_proto::{Action, IncomingMessage};
use tokio::sync::oneshot;

/// How long a write waits for the appliance's answer before it is assumed to have been taken.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// The appliance's answer to one write: `Err` carries the reason it was refused.
pub type Ack = Result<(), String>;

/// The writes on one connection that are waiting for an answer. Cheap to clone; clones share state.
#[derive(Debug, Clone, Default)]
pub struct WriteAcks {
    pending: Arc<Mutex<BTreeMap<u64, oneshot::Sender<Ack>>>>,
}

impl WriteAcks {
    /// An empty registry for one connection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts waiting for the answer to `msg_id`.
    pub(crate) fn register(&self, msg_id: u64) -> oneshot::Receiver<Ack> {
        let (sender, receiver) = oneshot::channel();
        self.lock().insert(msg_id, sender);
        receiver
    }

    /// Stops waiting for `msg_id`.
    pub(crate) fn forget(&self, msg_id: u64) {
        self.lock().remove(&msg_id);
    }

    /// Hands a received message to the write waiting for it. Returns whether one was.
    #[must_use = "a message no write was waiting for may still carry values"]
    pub fn resolve(&self, message: &IncomingMessage) -> bool {
        if message.action != Action::Response {
            return false;
        }
        let Some(sender) = message.msg_id.and_then(|id| self.lock().remove(&id)) else {
            return false;
        };
        let ack = match message.response_error() {
            Some((code, name)) if code >= 300 => {
                Err(format!("the appliance refused it ({code} {name})"))
            }
            _ => Ok(()),
        };
        // The writer may have stopped waiting; that is not an error here.
        let _gone = sender.send(ack);
        true
    }

    /// Fails every waiting write: the connection has ended.
    pub fn close(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, oneshot::Sender<Ack>>> {
        // A panic while holding the lock leaves the map itself consistent.
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
