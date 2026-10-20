//! One `msgID` allocator per connection: a second, independent counter for a connection must not be
//! constructible.
//!
//! `MsgIdAllocator::starting_at` is private, so it cannot be named from an integration test; proving
//! that negatively would need a compile-fail harness. Instead the runtime half asserts that every
//! handle from one `Handshake` is the same counter (and two `Handshake`s are not), and the source
//! half asserts the constructor is not public.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::path::PathBuf;

use hc_proto::{DeviceIdentity, Handshake, HandshakeConfig};

fn config() -> HandshakeConfig {
    HandshakeConfig {
        identity: DeviceIdentity {
            // Synthetic identity.
            name: "kessel-test".to_owned(),
            id: "00000000".to_owned(),
        },
    }
}

/// `crates/hc-proto/src/fsm.rs`, read as text.
fn fsm_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("fsm.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

#[test]
fn a_second_independent_allocator_cannot_be_constructed() {
    // (1) Every handle a `Handshake` hands out is the same counter: an id spent through one handle
    // is visible through another.
    let fsm = Handshake::new(config());
    let first = fsm.msg_ids();
    let second = fsm.msg_ids();
    let spent = first.take();
    assert_eq!(
        second.peek(),
        spent + 1,
        "two handles from one Handshake must be one counter: `first` spent {spent}, so `second` must \
         be positioned at {}, not {}",
        spent + 1,
        second.peek()
    );
    assert_eq!(
        fsm.msg_ids().take(),
        spent + 1,
        "and a handle taken later still continues the same sequence"
    );

    // (2) Non-vacuity: two DIFFERENT connections are two different counters, so assertion (1) is not
    // passing because `MsgIdAllocator` is secretly global.
    let other = Handshake::new(config());
    let mine = fsm.msg_ids();
    let theirs = other.msg_ids();
    let _burned = mine.take();
    assert_ne!(
        mine.peek(),
        theirs.peek(),
        "two Handshakes must not share a counter — a msgID sequence is per-connection \
         (message-model.md §5.1)"
    );

    // (3) The structural claim, as a source assertion (see the module docs).
    let source = fsm_source();
    // Assembled at runtime so this file does not itself contain the `pub` spelling it forbids.
    let sealed_decl = format!("fn starting_at{}", "(start: u64) -> Self");
    let public_decl = format!("pub fn starting_at{}", "(start: u64) -> Self");
    assert!(
        source.contains(&sealed_decl),
        "the constructor `{sealed_decl}` must exist in crates/hc-proto/src/fsm.rs for this assertion \
         to be watching anything at all"
    );
    assert!(
        !source.contains(&public_decl),
        "`MsgIdAllocator::starting_at` must NOT be public. A caller who can build one can build a \
         SECOND, independent counter for a connection that already has one, and re-issue every id the \
         session has already spent, so ids collide on the wire. The only \
         sanctioned source of an allocator is `Handshake::msg_ids`, which clones the handshake's own."
    );
    // `pub(crate)` is fine for the crate's own unit tests; nothing may cross the crate boundary.
    assert!(
        !source.contains("pub(super) fn starting_at"),
        "no crate-crossing spelling of the constructor may exist either"
    );
}
