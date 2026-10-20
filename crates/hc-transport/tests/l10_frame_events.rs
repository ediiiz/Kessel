//! Leak sweep over the frame-event payload (raw hex, a decoded header, chain state): no psk, iv,
//! serial, mac or haId may reach a trace event.
//!
//! * Needles are loaded at runtime, never hard-coded in this file.
//! * A leak is reported by needle source only, never by echoing the value.
//! * Non-vacuity: a sentinel is injected into a throwaway copy of the captured surface, and the
//!   surface's size is asserted against what was driven.
//!
//! # The needle set
//!
//! | source | why |
//! |---|---|
//! | the corpus's `synthetic_identifiers` | the deviceID / haId / mac / eNumber / ipAddress the replayed frames carry inside `data` |
//! | the corpus's `key.psk64` / `key.iv64` | key material |
//! | each frame's `plaintext_hex` prefix and `plaintext_utf8` | catches emitting the decrypted bytes where the contract says ciphertext |
//! | each frame's `mac_hex`, `chain.direction_hmac_after`, `chain.cbc_iv_after` | the rolling HMAC and CBC IV are key-derived; `chain` is an ordinal so this material stays inside the crate |
//!
//! Needles derived from `secrets/` are not swept here; that subset announces a skip on stderr rather
//! than passing silently. Everything is in-process; no appliance is contacted.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed asserts; indexing the corpus JSON by key is a test-only \
                  invariant — a missing key must panic loudly rather than silently load an empty \
                  needle set"
    )
)]

mod common;

use hc_proto::State;
use serde_json::Value;

/// A value that must not appear on any surface. `value` is never rendered by the sweep.
#[derive(Debug, Clone)]
struct Needle {
    source: &'static str,
    value: String,
}

/// Pushes `value` as a needle if it is long enough to be a genuine identifier rather than a token
/// that would collide by accident (every real needle here is ≥ 6 chars).
fn push_needle(needles: &mut Vec<Needle>, source: &'static str, value: &str) {
    if value.len() >= 6 {
        needles.push(Needle {
            source,
            value: value.to_owned(),
        });
    }
}

/// Every needle, all loaded at runtime.
fn needles(corpus_json: &str) -> Vec<Needle> {
    let corpus: Value = serde_json::from_str(corpus_json).expect("the golden corpus parses");
    let mut needles = Vec::new();

    if let Some(items) = corpus["synthetic_identifiers"].as_array() {
        for item in items {
            if let Some(value) = item["synthetic"].as_str() {
                push_needle(&mut needles, "corpus synthetic_identifiers", value);
            }
        }
    }
    push_needle(
        &mut needles,
        "corpus key",
        corpus["key"]["psk64"].as_str().unwrap_or_default(),
    );
    push_needle(
        &mut needles,
        "corpus key",
        corpus["key"]["iv64"].as_str().unwrap_or_default(),
    );

    for frame in corpus["frames"].as_array().unwrap_or(&Vec::new()) {
        // The decrypted bytes, in both encodings the corpus records them in. A frame event carries a
        // prefix of the CIPHERTEXT; emitting the plaintext instead is a one-character mistake.
        let plaintext_hex = frame["plaintext_hex"].as_str().unwrap_or_default();
        push_needle(
            &mut needles,
            "corpus frame plaintext (hex, first block)",
            plaintext_hex.get(..32).unwrap_or(plaintext_hex),
        );
        push_needle(
            &mut needles,
            "corpus frame plaintext (utf8)",
            frame["plaintext_utf8"].as_str().unwrap_or_default(),
        );
        // Key-derived chain material. `chain` is an ordinal so that none of this can reach a browser.
        push_needle(
            &mut needles,
            "corpus frame chain material",
            frame["mac_hex"].as_str().unwrap_or_default(),
        );
        for key in ["direction_hmac_after", "cbc_iv_after"] {
            push_needle(
                &mut needles,
                "corpus frame chain material",
                frame["chain"][key].as_str().unwrap_or_default(),
            );
        }
    }

    needles
}

/// Every surface/needle hit, described by source only — never by value.
fn find_leaks(surface: &[String], needles: &[Needle]) -> Vec<String> {
    let mut leaks = Vec::new();
    for needle in needles {
        if surface.iter().any(|text| text.contains(&needle.value)) {
            leaks.push(format!(
                "a needle from `{}` leaked into the frame-event surface",
                needle.source
            ));
        }
    }
    leaks
}

/// Announces a skipped env-gated subset on the process's real stderr.
///
/// Not `eprintln!`: libtest captures the `print!` family, so an `eprintln!` in a passing test is
/// invisible without `--nocapture`.
fn announce(message: &str) {
    use std::io::Write as _;
    let mut err = std::io::stderr();
    let _ignored = writeln!(err, "{message}");
    let _ignored = err.flush();
}

/// The sentinel. Not a needle from anywhere real: it exists only to prove the sweep can catch a hit.
const SENTINEL: &str = "KESSEL-FRAME-SWEEP-SENTINEL";

#[tokio::test]
async fn frame_events_emit_no_secret_values() {
    let corpus = common::corpus();
    let (sink, stream, device) = common::fake_wire();
    let mut session = common::client_session(&corpus, sink, stream);
    device.replay_rx(&corpus);

    // Capture EVERYTHING hc-transport emits, not only the frame events: a leak that lands in a plain
    // log line is just as much a leak, and the classifier would send it out as `event: log`.
    let (captured, guard) = common::capture_traces();
    let outcome = session
        .run_handshake()
        .await
        .expect("the golden corpus handshake completes");
    drop(guard);
    assert_eq!(outcome.state, State::Connected);

    let events = captured.events();
    let surface: Vec<String> = events
        .iter()
        .flat_map(common::CapturedEvent::as_swept_strings)
        .collect();

    // Non-vacuity of the capture: a sweep of almost nothing reports success. Every corpus frame
    // crosses a codec boundary, so one frame event per corpus frame must have arrived.
    let frame_count = captured.frames().len();
    assert_eq!(
        frame_count,
        corpus.frames.len(),
        "the swept surface is short: {frame_count} frame events for {} corpus frames. The sweep \
         below would run against a truncated payload and pass for the wrong reason.",
        corpus.frames.len()
    );
    assert!(
        surface.len() > frame_count,
        "each frame event must contribute several strings (target, message, one per field); \
         {} strings for {frame_count} events means `as_swept_strings` is not seeing the fields",
        surface.len()
    );

    let needles = needles(common::corpus_raw());
    assert!(
        needles.len() > 20,
        "only {} needles loaded — the corpus did not parse as expected, and a small \
         needle set is how a leak sweep passes without looking for anything",
        needles.len()
    );

    // NON-VACUITY of the SWEEP: plant the sentinel in a THROWAWAY copy, never in the real surface,
    // and prove the same matcher finds it there and not here.
    let mut throwaway = surface.clone();
    throwaway.push(format!("wire_head_hex={SENTINEL}"));
    let sentinel_needle = Needle {
        source: "injected sentinel",
        value: SENTINEL.to_owned(),
    };
    assert!(
        !find_leaks(&throwaway, std::slice::from_ref(&sentinel_needle)).is_empty(),
        "non-vacuity: the sweep MUST detect an injected sentinel, or the absence check below proves \
         nothing"
    );
    assert!(
        find_leaks(&surface, std::slice::from_ref(&sentinel_needle)).is_empty(),
        "the sentinel must exist only in the throwaway copy, never in the real surface"
    );

    // THE SWEEP.
    let leaks = find_leaks(&surface, &needles);
    assert!(
        leaks.is_empty(),
        "the frame-event payload leaks identifiers or key material (reported by needle source only, never by \
         value): {leaks:?}"
    );

    announce(
        "SKIP frame_events_emit_no_secret_values (secrets subset): this sweep does not read \
         secrets/, so secrets/-derived needles were NOT swept (the corpus identifiers, the corpus \
         key material, and the per-frame plaintext and chain material were).",
    );
}
