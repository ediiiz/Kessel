//! The bind, and the packaging property that keeps it safe.
//!
//! The daemon binds all interfaces because Home Assistant's ingress proxy connects to the
//! container's own IP on the internal network, not through its loopback. A wide bind is only safe
//! together with the packaging, so both halves are asserted:
//!
//! 1. the listener binds all interfaces, so ingress can reach it;
//! 2. `kessel/config.yaml` declares no `ports:` key, so nothing is published to the host: the socket
//!    is reachable from the container network and not from the LAN.
//!
//! The second is a text assertion over a YAML file because nothing in `apps/server` can enforce it,
//! and getting it wrong exposes a write-capable API to a LAN carrying a cooktop and an oven. No
//! non-loopback address of this host is probed, since that would put a socket on the appliance LAN.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::net::{IpAddr, Ipv4Addr};

mod common;

#[tokio::test]
async fn binds_all_interfaces_so_ingress_can_reach_it() {
    // Port 0 lets the OS choose a free port: this test must not depend on 8099 being free.
    let listener = server::serve::bind_api_listener(0)
        .await
        .expect("the daemon can bind a port");
    let bound = listener
        .local_addr()
        .expect("a bound listener has an address");

    assert_eq!(
        bound.ip(),
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        "the API listener bound {bound}; it must bind 0.0.0.0 so Home Assistant's ingress proxy — which \
         connects to the container's own IP, not through its loopback — can reach it"
    );
    assert!(
        !bound.ip().is_loopback(),
        "a loopback bind makes ingress return 502"
    );
    assert_ne!(bound.port(), 0, "the OS assigned a real port");

    // The advertised default agrees with what was bound, so the startup banner and the operator's
    // browser cannot disagree.
    assert_eq!(
        server::serve::api_address(server::serve::DEFAULT_PORT).ip(),
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        "the default API address must be the one that is actually bound"
    );
}

/// The add-on publishes no port, which no Rust code can enforce.
///
/// A `0.0.0.0` bind is safe here because the listener is reachable from the container network,
/// which ingress needs, and not from the LAN. A `ports:` key in `kessel/config.yaml` would expose a
/// write-capable API to every host on the appliance LAN.
#[test]
fn the_addon_publishes_no_port_to_the_host() {
    let path = common::workspace_root().join("kessel").join("config.yaml");
    let config = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    // Non-vacuity first: the file must be the one we think it is, or the absence below proves nothing.
    assert!(
        config.contains("ingress: true") && config.contains("ingress_port:"),
        "this must be the add-on config with the ingress keys in it, otherwise the assertion below is \
         vacuous: {}",
        path.display()
    );

    let published: Vec<&str> = config
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let bare = line.trim_start();
            !bare.starts_with('#') && bare.starts_with("ports:")
        })
        .collect();
    assert!(
        published.is_empty(),
        "kessel/config.yaml must declare NO `ports:` key. The daemon binds 0.0.0.0 so ingress can reach \
         it; publishing a port would put a write-capable API — one that can command a cooktop — on \
         the appliance LAN. Found: {published:?}"
    );
}
