//! The frame-transport seam the handshake is driven through.
//!
//! The bytes crossing it are wire frames (`ct ‖ mac` for AES); the crypto lives in the
//! [`crate::Driver`], not here.

use std::collections::VecDeque;

/// A transport error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SocketError {
    /// No more inbound frames: the fake queue is exhausted, or the real socket closed.
    #[error("socket closed / no more frames")]
    Closed,
}

/// A bidirectional frame transport. `recv` yields the next inbound wire frame; `send` takes an
/// outbound one.
pub trait Socket {
    /// The next inbound wire frame.
    ///
    /// # Errors
    ///
    /// [`SocketError::Closed`] when there is no next frame.
    fn recv(&mut self) -> Result<Vec<u8>, SocketError>;

    /// Sends one outbound wire frame.
    ///
    /// # Errors
    ///
    /// [`SocketError::Closed`] if the transport cannot accept it.
    fn send(&mut self, frame: &[u8]) -> Result<(), SocketError>;
}

/// A fake socket for replay tests: hands back a queued list of inbound frames and records
/// everything sent.
#[derive(Debug, Default)]
pub struct FakeSocket {
    inbound: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
}

impl FakeSocket {
    /// A fake socket that will hand back `inbound` in order.
    #[must_use]
    pub fn new(inbound: Vec<Vec<u8>>) -> Self {
        Self {
            inbound: inbound.into(),
            sent: Vec::new(),
        }
    }

    /// The wire frames sent so far, in send order.
    #[must_use]
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }
}

impl Socket for FakeSocket {
    fn recv(&mut self) -> Result<Vec<u8>, SocketError> {
        self.inbound.pop_front().ok_or(SocketError::Closed)
    }

    fn send(&mut self, frame: &[u8]) -> Result<(), SocketError> {
        self.sent.push(frame.to_vec());
        Ok(())
    }
}
