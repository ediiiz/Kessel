//! The TLS 1.2 PSK server side, mirroring what Kessel's client offers: cipher list `PSK`, no
//! certificate, any PSK identity.

use std::pin::Pin;

use base64::Engine as _;
use base64::alphabet::URL_SAFE;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use openssl::error::ErrorStack;
use openssl::ssl::{Ssl, SslContext, SslMethod, SslVerifyMode, SslVersion};
use tokio::net::TcpStream;
use tokio_openssl::SslStream;

use crate::SimError;

/// URL-safe base64 that also accepts non-zero trailing bits. The committed fixture profiles were
/// scrubbed character by character, so their keys are not canonical base64.
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// Decodes a profile `key` (URL-safe base64, padding optional, trailing bits ignored) to the raw
/// PSK.
///
/// # Errors
///
/// [`SimError::Key`] if the key is not URL-safe base64.
pub fn decode_psk(psk64: &str) -> Result<Vec<u8>, SimError> {
    LENIENT
        .decode(psk64.trim())
        .map_err(|_ignored| SimError::Key)
}

/// The canonical spelling of a profile `key`: the same PSK, re-encoded so a strict decoder (like
/// Kessel's) accepts it.
///
/// # Errors
///
/// [`SimError::Key`] if the key is not URL-safe base64.
pub fn canonical_key(psk64: &str) -> Result<String, SimError> {
    Ok(URL_SAFE_NO_PAD.encode(decode_psk(psk64)?))
}

/// A server context that answers every PSK identity with `psk`.
///
/// # Errors
///
/// [`SimError::Tls`] if OpenSSL refuses the configuration.
pub fn server_context(psk: Vec<u8>) -> Result<SslContext, SimError> {
    let tls = |error: ErrorStack| SimError::Tls(error.to_string());
    let mut builder = SslContext::builder(SslMethod::tls_server()).map_err(tls)?;
    builder
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .map_err(tls)?;
    builder
        .set_max_proto_version(Some(SslVersion::TLS1_2))
        .map_err(tls)?;
    builder.set_security_level(0);
    // Plain PSK and ECDHE-PSK; the certificate-based PSK families are never chosen, as there is
    // no certificate.
    builder.set_cipher_list("PSK").map_err(tls)?;
    builder.set_verify(SslVerifyMode::NONE);
    builder.set_psk_server_callback(move |_ssl, identity, psk_out| {
        let identity =
            identity.map_or_else(String::new, |raw| String::from_utf8_lossy(raw).into_owned());
        tracing::debug!(identity = %identity, "PSK identity offered");
        let out = psk_out.get_mut(..psk.len()).ok_or_else(ErrorStack::get)?;
        out.copy_from_slice(&psk);
        Ok(psk.len())
    });
    Ok(builder.build())
}

/// Runs the server side of the TLS handshake on an accepted socket.
///
/// # Errors
///
/// [`SimError::Tls`] if the handshake fails.
pub async fn accept(
    context: &SslContext,
    tcp: TcpStream,
) -> Result<SslStream<TcpStream>, SimError> {
    let ssl = Ssl::new(context).map_err(|error| SimError::Tls(error.to_string()))?;
    let mut stream = SslStream::new(ssl, tcp).map_err(|error| SimError::Tls(error.to_string()))?;
    Pin::new(&mut stream)
        .accept()
        .await
        .map_err(|error| SimError::Tls(error.to_string()))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "asserts may panic")]

    use super::*;

    #[test]
    fn decodes_an_unpadded_url_safe_key() {
        let raw = [0xFB_u8; 32];
        let psk = decode_psk(&URL_SAFE_NO_PAD.encode(raw)).unwrap();
        assert_eq!(psk, raw, "the key round-trips");
        let padded = format!("{}=", URL_SAFE_NO_PAD.encode(raw));
        assert_eq!(decode_psk(&padded).unwrap(), raw, "padding is optional");
    }

    #[test]
    fn tolerates_trailing_bits_and_canonicalises() {
        let canonical = URL_SAFE_NO_PAD.encode([0xFB_u8; 32]);
        // Flip the unused low bits of the last character.
        let mut sloppy = canonical.clone();
        sloppy.pop();
        sloppy.push('t');
        assert!(
            URL_SAFE_NO_PAD.decode(&sloppy).is_err(),
            "a strict decoder refuses it"
        );
        assert_eq!(
            canonical_key(&sloppy).unwrap(),
            canonical,
            "re-encoded canonically"
        );
    }

    #[test]
    fn rejects_a_key_that_is_not_base64() {
        assert!(decode_psk("not base64!").is_err(), "garbage is refused");
    }
}
