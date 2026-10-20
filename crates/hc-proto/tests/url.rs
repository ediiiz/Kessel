//! Appliance URL construction: pure string tests, no I/O.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_proto::{Transport, appliance_url, appliance_url_at};

/// A bare IPv6 literal gets exactly one bracket pair.
#[test]
fn ipv6_host_bracketed_once() {
    assert_eq!(
        appliance_url("2a0a::1", Transport::Aes).unwrap(),
        "ws://[2a0a::1]:80/homeconnect"
    );
}

/// Strip-then-wrap is idempotent: an already bracketed host is not double-bracketed.
#[test]
fn ipv6_already_bracketed_not_rewrapped() {
    assert_eq!(
        appliance_url("[2a0a::1]", Transport::Aes).unwrap(),
        "ws://[2a0a::1]:80/homeconnect"
    );
}

/// The zone id is preserved and percent-encoded per RFC 6874.
#[test]
fn ipv6_zone_id_preserved() {
    assert_eq!(
        appliance_url("fe80::1%eth0", Transport::Aes).unwrap(),
        "ws://[fe80::1%25eth0]:80/homeconnect"
    );
}

/// IPv4 and DNS names are untouched, and the transport — not the URL — fixes scheme and port.
#[test]
fn ipv4_and_dns_pass_through_with_transport_port() {
    assert_eq!(
        appliance_url("192.168.1.50", Transport::Aes).unwrap(),
        "ws://192.168.1.50:80/homeconnect"
    );
    assert_eq!(
        appliance_url("myhost", Transport::TlsPsk).unwrap(),
        "wss://myhost:443/homeconnect"
    );
}

/// An empty host (after trimming) is an error.
#[test]
fn empty_host_is_rejected() {
    assert!(appliance_url("   ", Transport::Aes).is_err());
}

// ---------------------------------------------------------------------------------------------
// Port override: an explicit port replaces the transport's default without changing host handling.
// ---------------------------------------------------------------------------------------------

/// `None` is exactly the transport's default port, so `appliance_url` and `appliance_url_at` agree.
#[test]
fn no_override_is_the_transport_default_port() {
    for (host, transport) in [
        ("192.168.1.50", Transport::Aes),
        ("myhost", Transport::TlsPsk),
        ("2a0a::1", Transport::Aes),
        ("fe80::1%eth0", Transport::TlsPsk),
    ] {
        assert_eq!(
            appliance_url_at(host, transport, None).unwrap(),
            appliance_url(host, transport).unwrap(),
            "an absent override must be indistinguishable from the transport's default"
        );
    }
}

/// An explicit port replaces the transport's, on both transports, without touching the scheme.
#[test]
fn an_explicit_port_replaces_the_transport_default() {
    assert_eq!(
        appliance_url_at("192.168.1.50", Transport::Aes, Some(8080)).unwrap(),
        "ws://192.168.1.50:8080/homeconnect"
    );
    assert_eq!(
        appliance_url_at("myhost", Transport::TlsPsk, Some(8443)).unwrap(),
        "wss://myhost:8443/homeconnect"
    );
}

/// The override composes with IPv6 bracketing rather than bypassing it; splicing the port in on the
/// last `:` would corrupt exactly this case.
#[test]
fn an_explicit_port_still_brackets_ipv6() {
    assert_eq!(
        appliance_url_at("2a0a::1", Transport::Aes, Some(8080)).unwrap(),
        "ws://[2a0a::1]:8080/homeconnect"
    );
    assert_eq!(
        appliance_url_at("fe80::1%eth0", Transport::TlsPsk, Some(8443)).unwrap(),
        "wss://[fe80::1%25eth0]:8443/homeconnect"
    );
}

/// An empty host is still rejected when a port is supplied — the override adds no bypass.
#[test]
fn empty_host_is_rejected_with_an_override_too() {
    assert!(appliance_url_at("   ", Transport::Aes, Some(8080)).is_err());
}
