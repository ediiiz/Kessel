//! Crypto failures.
//!
//! Every one of these is a hard error: once the RX chain's position is uncertain every later
//! frame fails too, so the caller closes the socket and reconnects.

use thiserror::Error;

/// Everything `hc-crypto` can refuse to do.
///
/// Variants carry lengths and indices but never key material, plaintext or ciphertext.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CryptoError {
    /// `psk64` or `iv64` was not decodable URL-safe base64.
    #[error("key material is not valid URL-safe base64: {0}")]
    Base64(String),

    /// The psk decoded to something other than 32 bytes.
    #[error("psk must be 32 bytes, got {got}")]
    PskLength {
        /// The decoded length that was rejected.
        got: usize,
    },

    /// The iv decoded to something other than the 16-byte AES block size.
    #[error("iv must be 16 bytes, got {got}")]
    IvLength {
        /// The decoded length that was rejected.
        got: usize,
    },

    /// The inbound frame was shorter than [`crate::MIN_FRAME_LEN`].
    #[error("frame is {len} bytes, shorter than the 32-byte minimum")]
    FrameTooShort {
        /// The rejected frame length.
        len: usize,
    },

    /// The inbound frame length was not a multiple of the AES block size.
    ///
    /// An unaligned frame cannot be CBC-decrypted, so this is fatal rather than a warning.
    #[error("frame is {len} bytes, not a multiple of the 16-byte block size")]
    FrameNotBlockAligned {
        /// The rejected frame length.
        len: usize,
    },

    /// The decrypted buffer was empty, so it cannot carry the two structural pad bytes.
    #[error("decrypted plaintext is empty; every padded frame carries at least two pad bytes")]
    EmptyPlaintext,

    /// An injected pad does not have the required shape.
    ///
    /// Only reachable through [`crate::Sealer::seal_with_pad`]; a pad built by
    /// [`crate::build_pad`] is always valid. Checked up front because a misaligned pad would
    /// otherwise surface as an unrelated failure several frames later.
    #[error(
        "an injected {pad_len}-byte pad for {clear_len} clear bytes is not a valid pad: its length \
         must be 2..=16 or 17, it must start with 0x00, end with its own length, and bring the \
         total to a multiple of 16"
    )]
    InjectedPad {
        /// Length of the plaintext the pad was offered for.
        clear_len: usize,
        /// Length of the offered pad.
        pad_len: usize,
    },

    /// The MAC on the frame did not match the recomputed one.
    ///
    /// Carries neither tag: the computed tag would hand an attacker the answer. The RX chain has
    /// not advanced, so the connection must be dropped rather than retried in place.
    #[error("frame MAC does not match the recomputed HMAC")]
    HmacMismatch,

    /// The decrypted plaintext was not valid UTF-8.
    #[error("decrypted plaintext is not valid UTF-8 (valid up to byte {valid_up_to})")]
    Utf8 {
        /// How many leading bytes were valid UTF-8.
        valid_up_to: usize,
    },

    /// The trailing pad-length byte is `0` or larger than the buffer.
    #[error("pad length byte {pad_len} is out of range for a {padded_len}-byte plaintext")]
    PadLength {
        /// The value of the trailing byte.
        pad_len: u8,
        /// The length of the buffer it was read from.
        padded_len: usize,
    },
}
