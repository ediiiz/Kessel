//! AES app-layer crypto for the local Home Connect protocol: the HMAC-SHA256 KDF, the stateful
//! CBC and rolling HMAC chains, and the custom (non-PKCS#7) padding. No I/O, no async.
//!
//! In the order a connection uses it:
//!
//! 1. [`SessionKeys::from_base64`] decodes `psk64`/`iv64` and derives `enckey` and `mackey`.
//! 2. [`AesSession::new`] with a [`Role`] builds one [`Sealer`] and one [`Opener`].
//! 3. [`AesSession::seal`] with a [`PadSource`] sends; [`AesSession::open`] receives.
//! 4. [`AesSession::reset`] on every reconnect, because chain state is per connection.
//!
//! Pad randomness is injected through [`PadSource`]. Every [`CryptoError`] is fatal to the
//! connection; closing and reconnecting is the caller's job.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

mod chain;
mod error;
mod keys;
mod pad;
mod secret;

pub use chain::{
    AesSession, ChainState, Direction, MAC_LEN, MIN_FRAME_LEN, OpenedFrame, Opener, Role,
    SealedFrame, Sealer,
};
pub use error::CryptoError;
pub use keys::{IV_LEN, PSK_LEN, SessionKeys, decode_key};
pub use pad::{BLOCK_LEN, PadSource, build_pad, pad_len, unpad};
pub use secret::{REDACTED, Secret};

/// The crate's own name, as it appears in `[workspace.members]`.
pub const CRATE_NAME: &str = "hc-crypto";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(
            CRATE_NAME, "hc-crypto",
            "crate name is part of the workspace layout"
        );
    }
}
