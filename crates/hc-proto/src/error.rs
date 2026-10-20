//! The crate's error type.
//!
//! A `code` on a RESPONSE is an application error, not a decode failure, so it is carried as
//! [`ProtoError::Response`] with the numeric value preserved.

use crate::codes::ResponseCode;

/// Anything that can go wrong turning wire bytes into a handshake, or driving it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtoError {
    /// A frame did not parse as JSON, even after the `]"` → `]` repair.
    #[error("frame did not decode as JSON: {0}")]
    Decode(String),

    /// A required field was absent or the wrong shape (e.g. `POST /ei/initialValues` with no
    /// `data[0].edMsgID`).
    #[error("malformed handshake message: {0}")]
    Malformed(String),

    /// A response carried a `code` on a step where that is fatal (`/ci/services`,
    /// `/ci/authentication`).
    #[error("{resource} -> code {} ({name})", .code)]
    Response {
        /// The resource the failing response was for.
        resource: String,
        /// The raw numeric code, preserved even when unrecognised.
        code: i64,
        /// The typed classification of `code`.
        name: ResponseCode,
    },

    /// A feature-write `value` the wire model cannot represent (see [`crate::WriteValue`]).
    ///
    /// Only scalar values have a known encoding; `null`, arrays and objects are refused rather
    /// than sent to firmware that parses positionally.
    #[error("a write value of JSON type {0} cannot be represented on the wire")]
    UnrepresentableWriteValue(&'static str),

    /// A feature write was asked for with no items ([`crate::OutgoingMessage::values_write`]).
    ///
    /// An empty list would put `"data":[]` on the wire, a shape with no known firmware behaviour,
    /// and an empty write asks for nothing, so it is refused.
    #[error("a feature write needs at least one item; an empty write has no attested wire form")]
    EmptyWriteItemList,

    /// The FSM received a message that does not belong in its current state.
    #[error("unexpected {action} {resource} in state {state}")]
    Unexpected {
        /// The action that arrived.
        action: String,
        /// The resource that arrived.
        resource: String,
        /// The state the FSM was in.
        state: String,
    },
}
