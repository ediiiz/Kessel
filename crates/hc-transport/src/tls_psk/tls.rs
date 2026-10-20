//! The TLS-PSK tunnel: the OpenSSL connector, the PSK client callback, and the async dial.

use std::pin::Pin;

use hc_crypto::Secret;
use openssl::error::ErrorStack;
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode, SslVersion};
use tokio::net::TcpStream;

/// The OpenSSL cipher list. Required: the builder's default list has no PSK suites and fails with
/// the misleading `unknown cipher returned`. `"PSK"` selects both the ECDHE-PSK and plain PSK
/// families.
const PSK_CIPHER_LIST: &str = "PSK";

/// The OpenSSL message when no PSK cipher was configured; see [`TlsPskError::MissingPskCipher`].
const UNKNOWN_CIPHER_SIGNATURE: &str = "unknown cipher returned";

/// The TLS-PSK pre-shared key and client identity used for the handshake.
///
/// The `psk` is a [`Secret`] that leaves only into the OpenSSL callback and is never logged. The
/// `identity` is the TLS-PSK identity (`""` or `"HCCOM_Local_App"`), not the app-layer
/// [`hc_proto::DeviceIdentity`].
pub struct PskConfig {
    identity: Vec<u8>,
    psk: Secret<Vec<u8>>,
}

impl PskConfig {
    /// Builds a config from a TLS-PSK identity and the raw 32-byte pre-shared key.
    ///
    /// The psk is used directly as the TLS PSK, with no key derivation. For an untested appliance,
    /// try the identity `""` first, then `"HCCOM_Local_App"`.
    #[must_use]
    pub fn new(identity: impl Into<Vec<u8>>, psk: Secret<Vec<u8>>) -> Self {
        Self {
            identity: identity.into(),
            psk,
        }
    }
}

impl core::fmt::Debug for PskConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The psk never renders; only the identity's length is shown.
        f.debug_struct("PskConfig")
            .field("identity_len", &self.identity.len())
            .field("psk", &"<redacted>")
            .finish()
    }
}

/// Everything the TLS-PSK tunnel can refuse to do while being established.
///
/// OpenSSL errors are stringified so no key material survives into the message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TlsPskError {
    /// The TLS handshake failed with `unknown cipher returned`: no PSK cipher was offered. This
    /// crate always sets one, so this indicates a regression in [`psk_connector`].
    #[error(
        "TLS handshake failed with `unknown cipher returned`: no PSK cipher was offered — \
         set_cipher_list(\"PSK\") must be called (tls-psk.md §245)"
    )]
    MissingPskCipher,

    /// A TCP connection to the appliance could not be established.
    #[error("TCP connect to the appliance failed: {0}")]
    Connect(String),

    /// Any other OpenSSL configuration or handshake failure, stringified (never carries the psk).
    #[error("TLS-PSK error: {0}")]
    Ssl(String),
}

fn ssl_err(error: &ErrorStack) -> TlsPskError {
    TlsPskError::Ssl(error.to_string())
}

/// Classifies a handshake error message, naming the missing-PSK-cipher case.
fn classify_signature(text: &str) -> TlsPskError {
    if text.contains(UNKNOWN_CIPHER_SIGNATURE) {
        TlsPskError::MissingPskCipher
    } else {
        TlsPskError::Ssl(text.to_owned())
    }
}

/// Classifies a TLS handshake error, naming the missing-PSK-cipher case.
fn classify_handshake_error(error: &openssl::ssl::Error) -> TlsPskError {
    classify_signature(&error.to_string())
}

/// Writes the identity and psk into OpenSSL's callback buffers.
///
/// `identity_out` receives a NUL-terminated C string (a single zero byte for the empty identity),
/// and the return value is the psk length, not the identity length. A too-small buffer is an
/// error, not a panic, since the callback must not unwind across the C boundary.
fn write_psk(
    identity: &[u8],
    psk: &[u8],
    identity_out: &mut [u8],
    psk_out: &mut [u8],
) -> Result<usize, ErrorStack> {
    let id_dst = identity_out
        .get_mut(..identity.len())
        .ok_or_else(ErrorStack::get)?;
    id_dst.copy_from_slice(identity);
    let terminator = identity_out
        .get_mut(identity.len())
        .ok_or_else(ErrorStack::get)?;
    *terminator = 0;
    let psk_dst = psk_out.get_mut(..psk.len()).ok_or_else(ErrorStack::get)?;
    psk_dst.copy_from_slice(psk);
    Ok(psk.len())
}

/// Builds the TLS-PSK [`SslConnector`].
///
/// TLS 1.2 is pinned as both minimum and maximum, since 1.3 has no usable external-PSK path.
/// Security level 0 is set through its own API call rather than an `@SECLEVEL=0` cipher suffix.
/// Certificate verification is off; the appliance presents none.
///
/// # Errors
///
/// [`TlsPskError`] if any OpenSSL configuration call fails.
pub fn psk_connector(config: &PskConfig) -> Result<SslConnector, TlsPskError> {
    let mut builder =
        SslConnector::builder(SslMethod::tls_client()).map_err(|error| ssl_err(&error))?;
    builder
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .map_err(|error| ssl_err(&error))?;
    builder
        .set_max_proto_version(Some(SslVersion::TLS1_2))
        .map_err(|error| ssl_err(&error))?;
    builder.set_security_level(0);
    builder
        .set_cipher_list(PSK_CIPHER_LIST)
        .map_err(|error| ssl_err(&error))?;
    builder.set_verify(SslVerifyMode::NONE);

    let identity = config.identity.clone();
    // The only place the psk leaves its `Secret`; it is never logged and never enters an error.
    let psk = config.psk.expose_secret().clone();
    builder.set_psk_client_callback(move |_ssl, _hint, identity_out, psk_out| {
        write_psk(&identity, &psk, identity_out, psk_out)
    });
    Ok(builder.build())
}

/// Dials `host:port` over TCP and completes the TLS-PSK handshake, returning the async TLS stream.
///
/// The WebSocket upgrade runs on top of the returned stream. SNI and hostname verification are
/// off, so the domain passed to [`openssl::ssl::ConnectConfiguration::into_ssl`] is unused.
///
/// # Errors
///
/// [`TlsPskError::Connect`] if TCP fails; [`TlsPskError::MissingPskCipher`] or [`TlsPskError::Ssl`]
/// if the TLS handshake fails.
pub async fn tls_connect_stream(
    host: &str,
    port: u16,
    config: &PskConfig,
) -> Result<tokio_openssl::SslStream<TcpStream>, TlsPskError> {
    let connector = psk_connector(config)?;
    let ssl = connector
        .configure()
        .map_err(|error| ssl_err(&error))?
        .verify_hostname(false)
        .use_server_name_indication(false)
        .into_ssl(host)
        .map_err(|error| ssl_err(&error))?;
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|error| TlsPskError::Connect(error.to_string()))?;
    let mut stream = tokio_openssl::SslStream::new(ssl, tcp).map_err(|error| ssl_err(&error))?;
    Pin::new(&mut stream)
        .connect()
        .await
        .map_err(|error| classify_handshake_error(&error))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    #![cfg_attr(
        test,
        allow(
            clippy::unwrap_used,
            clippy::expect_used,
            clippy::panic,
            reason = "test asserts may panic; library code may not"
        )
    )]

    use super::*;

    #[test]
    fn write_psk_nul_terminates_empty_identity_and_returns_psk_len() {
        // The empty identity is a single zero byte, not nothing.
        let mut id_out = [0xAA_u8; 8];
        let mut psk_out = [0_u8; 32];
        let psk = [7_u8; 32];
        let written = write_psk(b"", &psk, &mut id_out, &mut psk_out).unwrap();
        assert_eq!(
            written, 32,
            "the return value is the psk length, not the identity length"
        );
        assert_eq!(
            id_out.first(),
            Some(&0),
            "the empty identity is a single NUL byte"
        );
        assert_eq!(psk_out, psk, "the psk is copied verbatim");
    }

    #[test]
    fn write_psk_nul_terminates_nonempty_identity() {
        let mut id_out = [0xAA_u8; 32];
        let mut psk_out = [0_u8; 32];
        let id = b"HCCOM_Local_App";
        write_psk(id, &[1_u8; 32], &mut id_out, &mut psk_out).unwrap();
        assert_eq!(
            id_out.get(..id.len()),
            Some(&id[..]),
            "identity bytes are copied"
        );
        assert_eq!(
            id_out.get(id.len()),
            Some(&0),
            "a NUL terminator follows the identity (tls-psk.md §245)"
        );
    }

    #[test]
    fn write_psk_errors_instead_of_panicking_on_short_buffer() {
        // identity.len() + 1 must fit: an exactly-len buffer has no room for the terminator.
        let mut id_out = [0_u8; 3];
        let mut psk_out = [0_u8; 32];
        assert!(
            write_psk(b"abc", &[0_u8; 32], &mut id_out, &mut psk_out).is_err(),
            "a buffer with no room for the NUL terminator is an error, not a panic"
        );
    }

    #[test]
    fn classify_names_the_missing_psk_cipher_case() {
        // The OpenSSL message seen when no PSK cipher is offered maps to the named error.
        let signature = "error:0A0000F8:SSL routines:set_client_ciphersuite:unknown cipher returned:\
             ssl/statem/statem_clnt.c:1400";
        assert!(
            matches!(classify_signature(signature), TlsPskError::MissingPskCipher),
            "the `unknown cipher returned` signature is named MissingPskCipher"
        );
        assert!(
            matches!(
                classify_signature("some other tls error"),
                TlsPskError::Ssl(_)
            ),
            "an unrelated handshake error stays generic"
        );
    }
}
