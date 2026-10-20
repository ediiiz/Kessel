//! Build gate: the `tls-psk` feature compiles and statically links the vendored OpenSSL. No socket
//! is opened. With the feature off this file compiles to nothing.

#![cfg(feature = "tls-psk")]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

use hc_transport::tls_psk;

#[test]
fn tls_psk_build_links_vendored_openssl() {
    // PRIMARY, host-independent proof: the `vendored` feature statically links libssl.a/libcrypto.a
    // into this binary, so no system libssl.so/libcrypto.so is dynamically mapped.
    assert!(
        tls_psk::links_vendored_openssl(),
        "the tls-psk build must statically link vendored OpenSSL (no dynamic libssl/libcrypto)"
    );

    // The linked OpenSSL is callable — proves the openssl crate is genuinely linked in.
    let version = tls_psk::openssl_version();
    assert!(
        version.starts_with("OpenSSL "),
        "linked OpenSSL version() is callable: {version}"
    );

    // A vendored build's dir() reports openssl-src's upstream compile-time OPENSSLDIR, not a
    // target/ path, so it cannot tell vendored from system OpenSSL; the static-link check can.
    let dir = tls_psk::openssl_dir();
    assert!(
        !dir.contains("target/"),
        "observed: vendored dir() is the upstream default, not a target/ path: {dir}"
    );
}
