//! Appliance WebSocket URL construction.
//!
//! Strip-then-wrap makes the operation idempotent (an already-bracketed host is not re-wrapped),
//! a zone id is percent-encoded per RFC 6874 (`%eth0` → `%25eth0`), and a host with no `:` (IPv4
//! or a DNS name) is never bracketed. The transport, not the host, fixes scheme and port.

use crate::error::ProtoError;

/// Which transport a URL is for; it fixes scheme and port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// App-layer AES: `ws://<host>:80/homeconnect`.
    Aes,
    /// TLS-PSK: `wss://<host>:443/homeconnect`.
    TlsPsk,
}

impl Transport {
    const fn scheme(self) -> &'static str {
        match self {
            Self::Aes => "ws",
            Self::TlsPsk => "wss",
        }
    }

    /// The port this transport uses when no override is given.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Aes => 80,
            Self::TlsPsk => 443,
        }
    }
}

/// Builds `<scheme>://<host>:<port>/homeconnect` for a configured host.
///
/// IPv4 and DNS names pass through untouched; a bare IPv6 literal is bracketed exactly once; an
/// already-bracketed literal is not re-wrapped; a link-local zone id is percent-encoded.
///
/// # Errors
///
/// [`ProtoError::Malformed`] if the host is empty after trimming.
pub fn appliance_url(host: &str, transport: Transport) -> Result<String, ProtoError> {
    appliance_url_at(host, transport, None)
}

/// [`appliance_url`] with an optional port override.
///
/// `None` uses the transport's default port and matches [`appliance_url`]. Only the port can be
/// overridden; the scheme always follows the transport.
///
/// # Errors
///
/// [`ProtoError::Malformed`] if the host is empty after trimming.
pub fn appliance_url_at(
    host: &str,
    transport: Transport,
    port: Option<u16>,
) -> Result<String, ProtoError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(ProtoError::Malformed("empty appliance host".to_owned()));
    }

    // Unwrap one surrounding bracket pair first, so wrapping is idempotent.
    let unwrapped = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);

    // A `:` in the unwrapped host means it is an IPv6 literal.
    let formatted_host = if unwrapped.contains(':') {
        // RFC 6874: percent-encode the zone-id delimiter, if present.
        let encoded = unwrapped.replacen('%', "%25", 1);
        format!("[{encoded}]")
    } else {
        unwrapped.to_owned()
    };

    // The port is appended after the host has been bracketed, never spliced into a finished string.
    Ok(format!(
        "{}://{}:{}/homeconnect",
        transport.scheme(),
        formatted_host,
        port.unwrap_or_else(|| transport.default_port())
    ))
}
