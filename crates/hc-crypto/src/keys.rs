//! Key material and the KDF: two single-shot HMACs, no HKDF, salt or info string.
//!
//! ```text
//! enckey = HMAC_SHA256(psk, "ENC")   // 32 bytes -> the AES-256 key
//! mackey = HMAC_SHA256(psk, "MAC")   // 32 bytes -> the HMAC key
//! ```
//!
//! The `iv` is not derived. It is used raw as the initial CBC IV for both directions and as a
//! constant prefix in every HMAC computation.

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use hmac::{Hmac, Mac as _};
use sha2::Sha256;

use crate::error::CryptoError;
use crate::secret::Secret;

type HmacSha256 = Hmac<Sha256>;

/// Length of the pre-shared key, in bytes.
pub const PSK_LEN: usize = 32;
/// Length of the iv, in bytes (the AES block size).
pub const IV_LEN: usize = 16;

/// The derived, per-appliance key material for one AES session.
///
/// Immutable and free of chain state, so it survives the per-connection reset.
#[derive(Debug, Clone)]
pub struct SessionKeys {
    enc_key: Secret<[u8; 32]>,
    mac_key: Secret<[u8; 32]>,
    iv: Secret<[u8; IV_LEN]>,
}

impl SessionKeys {
    /// Decodes `psk64`/`iv64` from the appliance profile and derives the two keys.
    ///
    /// The encoding is URL-safe base64 with optional `=` padding.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Base64`] if either input is not URL-safe base64;
    /// [`CryptoError::PskLength`] / [`CryptoError::IvLength`] if a decoded length is wrong.
    pub fn from_base64(psk64: &str, iv64: &str) -> Result<Self, CryptoError> {
        let psk = decode_b64url(psk64)?;
        let iv = decode_b64url(iv64)?;
        Self::from_bytes(&psk, &iv)
    }

    /// Derives the two keys from already-decoded material.
    ///
    /// # Errors
    ///
    /// [`CryptoError::PskLength`] if `psk` is not 32 bytes, [`CryptoError::IvLength`] if `iv`
    /// is not 16.
    pub fn from_bytes(psk: &[u8], iv: &[u8]) -> Result<Self, CryptoError> {
        let psk: [u8; PSK_LEN] = psk
            .try_into()
            .map_err(|_ignored| CryptoError::PskLength { got: psk.len() })?;
        let iv: [u8; IV_LEN] = iv
            .try_into()
            .map_err(|_ignored| CryptoError::IvLength { got: iv.len() })?;
        let (enc_key, mac_key) = derive(&psk);
        Ok(Self {
            enc_key: Secret::new(enc_key),
            mac_key: Secret::new(mac_key),
            iv: Secret::new(iv),
        })
    }

    /// The AES-256-CBC key, `HMAC_SHA256(psk, "ENC")`.
    #[must_use]
    pub const fn enc_key(&self) -> &Secret<[u8; 32]> {
        &self.enc_key
    }

    /// The HMAC-SHA256 key, `HMAC_SHA256(psk, "MAC")`.
    #[must_use]
    pub const fn mac_key(&self) -> &Secret<[u8; 32]> {
        &self.mac_key
    }

    /// The raw iv, used both as the initial CBC IV and as the constant HMAC prefix.
    #[must_use]
    pub const fn iv(&self) -> &Secret<[u8; IV_LEN]> {
        &self.iv
    }
}

/// Decodes a profile's `key` or `iv`.
///
/// # Errors
///
/// [`CryptoError::Base64`] if the text is not base64.
pub fn decode_key(value: &str) -> Result<Vec<u8>, CryptoError> {
    decode_b64url(value)
}

/// Base64 as profile exporters write it: URL-safe or standard alphabet, padded or not, and
/// tolerant of non-zero leftover bits in the final character.
fn decode_b64url(value: &str) -> Result<Vec<u8>, CryptoError> {
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    let normalised: String = value
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    LENIENT
        .decode(normalised)
        .map_err(|error| CryptoError::Base64(error.to_string()))
}

/// The whole KDF: two single-shot HMACs over ASCII labels.
fn derive(psk: &[u8; PSK_LEN]) -> ([u8; 32], [u8; 32]) {
    (hmac_sha256(psk, b"ENC"), hmac_sha256(psk, b"MAC"))
}

/// A full, untruncated `HMAC_SHA256` digest.
#[expect(
    clippy::expect_used,
    reason = "HMAC accepts a key of any length, so new_from_slice cannot fail"
)]
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}
