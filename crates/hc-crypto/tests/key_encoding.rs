//! Profile keys decode however the exporting tool wrote the base64.

#![cfg_attr(
    test,
    allow(clippy::unwrap_used, reason = "tests may panic on failed assertions")
)]

use hc_crypto::SessionKeys;

/// 32 bytes of zeros whose final character carries leftover bits, as a tool that encodes by hand
/// (or a scrubbed fixture) may produce.
fn non_canonical_psk() -> String {
    format!("{}B", "A".repeat(42))
}

#[test]
fn a_key_with_leftover_bits_decodes() {
    let result = SessionKeys::from_base64(&non_canonical_psk(), &"A".repeat(22));
    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn a_padded_or_standard_alphabet_key_decodes() {
    let psk = format!("{}=", "_-".repeat(22).chars().take(43).collect::<String>());
    assert!(SessionKeys::from_base64(&psk, &"A".repeat(22)).is_ok());
    let standard = psk.replace('_', "/").replace('-', "+");
    assert!(SessionKeys::from_base64(&standard, &"A".repeat(22)).is_ok());
}

#[test]
fn hc_crypto_decode_key_is_the_same_decoder() {
    assert_eq!(
        hc_crypto::decode_key(&non_canonical_psk()).unwrap().len(),
        32
    );
}
