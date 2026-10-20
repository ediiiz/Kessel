//! `Secret<T>`: redaction as a type.
//!
//! The `psk`/`key`, `iv`, `serialNumber`, `mac`, `shipSki` and the appliance `deviceID`/`haId`
//! must never appear in logs, MQTT topics or SSE frames. `Secret<T>` redacts them in `Debug`,
//! `Display` and `serde`, so a leak needs one of three explicit, greppable calls:
//! [`Secret::expose_secret`], [`Secret::into_exposed`] and [`Secret::map`].
//!
//! It lives here because `hc-crypto` has no internal dependencies and every other crate needs it.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The one and only rendering a secret ever has on the way out of the process.
///
/// Tests assert on the substring `redacted`, so keep that stem if this is ever restyled.
pub const REDACTED: &str = "<redacted>";

/// A value that cannot leak through `Debug`, `Display` or `serde`.
///
/// ```
/// use hc_crypto::Secret;
///
/// let psk = Secret::new("a-pre-shared-key".to_owned());
/// assert_eq!(format!("{psk}"), "<redacted>");
/// assert_eq!(format!("{psk:?}"), "Secret(<redacted>)");
/// assert_eq!(serde_json::to_string(&psk).unwrap(), "\"<redacted>\"");
/// assert_eq!(psk.expose_secret(), "a-pre-shared-key");
/// ```
/// # Why there is no `PartialEq`
///
/// A derived `==` would be a non-constant-time comparison of secret values. If a comparison is
/// ever needed, use `subtle::ConstantTimeEq`.
#[derive(Clone, Copy, Default)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps a value so it can no longer be rendered or serialised.
    #[must_use]
    pub const fn new(inner: T) -> Self {
        Self(inner)
    }

    /// Borrows the protected value.
    ///
    /// Deliberately verbose so every read of a secret is visible in review. See also
    /// [`Self::into_exposed`] and [`Self::map`].
    #[must_use]
    pub const fn expose_secret(&self) -> &T {
        &self.0
    }

    /// Consumes the wrapper and returns the protected value.
    #[must_use]
    pub fn into_exposed(self) -> T {
        self.0
    }

    /// Applies a function to the protected value, returning a `Secret` of the result.
    ///
    /// Lets a secret be transformed (base64-decoded, key-derived) without ever being unwrapped
    /// into an unprotected binding.
    #[must_use]
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Secret<U> {
        Secret(f(self.0))
    }
}

impl<T> From<T> for Secret<T> {
    fn from(inner: T) -> Self {
        Self(inner)
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Not `f.debug_tuple(..)`: that would render the payload under `{:#?}`.
        write!(f, "Secret({REDACTED})")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> Serialize for Secret<T> {
    /// Serialises as the redaction marker, never as the payload.
    ///
    /// # Errors
    ///
    /// Propagates whatever the serializer returns for a string.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    /// Redaction is outbound-only: config files and profile JSON carry real secrets inbound.
    ///
    /// # Errors
    ///
    /// Propagates the inner type's deserialisation error.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}
