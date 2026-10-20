//! Runtime leak sweep: drive a complete session (the real hc-crypto/hc-proto handshake over the
//! golden corpus, the supervisor lifecycle, and the MQTT bridge) against a fake transport and a
//! fake broker, capture every emitted surface, and assert that no appliance identifier or key
//! material appears on any of them.
//!
//! Captured surfaces:
//!
//! * tracing / trace-bus output, at `TRACE` level, so a `trace!`/`debug!` leak is caught too;
//! * every MQTT publish, topic and payload, including every HA discovery config;
//! * every subscription filter;
//! * the LWT / will.
//!
//! Needles, all loaded at runtime:
//!
//! * the synthetic corpus identifiers and `key.psk64`/`key.iv64` from
//!   `fixtures/aes/hob-session.json`;
//! * the scrubbed hob profile's own haId / serial / mac;
//! * the real identifiers from `secrets/`, when `KESSEL_RAW_PROFILES` points at them, otherwise a
//!   logged skip.
//!
//! A leak is reported by needle source and surface name only, never by value. SSE frames are swept
//! separately in `apps/server/tests/l10_sse_sweep.rs`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions and index known corpus positions"
    )
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use hc_core::backoff::Backoff;
use hc_core::bridge::DeviceBridge;
use hc_core::device::{ConnectionState, DeviceRole};
use hc_core::discovery::{Announcement, DiscoveryMode};
use hc_core::mapping::{DeviceId, DeviceMapping, Excludes};
use hc_core::mqtt::{LastWill, MqttClient, MqttError};
use hc_core::shadow::Shadow;
use hc_core::supervisor::{DeviceConfig, DeviceLink, DeviceSession, LinkError, run_device};
use hc_core::trace::{TraceBus, TraceBusLayer};
use hc_crypto::{AesSession, PadSource, Role, SessionKeys};
use hc_profile::Profile;
use hc_proto::{
    Clock, DeviceIdentity as ProtoIdentity, Driver, DriverSources, FakeSocket, Handshake,
    HandshakeConfig, NonceSource, State, ValueUpdate,
};
use tracing_subscriber::layer::SubscriberExt as _;

// =============================================================================================
// Paths — everything read at runtime, relative to the workspace root.
// =============================================================================================

/// The workspace root (`crates/hc-core/../..`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The golden AES corpus.
fn corpus_path() -> PathBuf {
    workspace_root()
        .join("fixtures")
        .join("aes")
        .join("hob-session.json")
}

/// The scrubbed hob profile, in `hc-profile`'s test tree.
fn hob_zip_path() -> PathBuf {
    workspace_root()
        .join("crates")
        .join("hc-profile")
        .join("tests")
        .join("fixtures")
        .join("hob-aes.zip")
}

// =============================================================================================
// Corpus-backed link: the REAL hc-crypto + hc-proto handshake, driven over the golden corpus.
// =============================================================================================

/// A time source fixed at 0; the replay needs no real timeout.
struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ms(&self) -> u64 {
        0
    }
}

/// Streams the recorded pad middles in tx order so each `seal` reproduces the corpus ciphertext.
struct PadStream {
    bytes: Vec<u8>,
    pos: usize,
}
impl PadSource for PadStream {
    fn fill_pad(&mut self, out: &mut [u8]) {
        let end = self.pos + out.len();
        out.copy_from_slice(&self.bytes[self.pos..end]);
        self.pos = end;
    }
}

/// Yields the recorded `/ci/authentication` nonce verbatim so that tx frame is byte-exact.
struct RecordedNonce(String);
impl NonceSource for RecordedNonce {
    fn next_nonce(&mut self) -> String {
        self.0.clone()
    }
}

/// A live session that seeds no `/ro` values (the idle hob sends none) and closes cleanly, so the
/// supervisor completes one full connect→steady→reconnect cycle.
struct CorpusSession;
#[async_trait]
impl DeviceSession for CorpusSession {
    fn initial_state(&self) -> Vec<ValueUpdate> {
        Vec::new()
    }
    async fn run_steady_state(
        &mut self,
        _sink: &mut dyn hc_core::supervisor::UpdateSink,
    ) -> Result<(), LinkError> {
        Ok(())
    }
}

/// A [`DeviceLink`] whose `connect` runs the whole golden-corpus handshake through the real
/// `hc-crypto` chain and `hc-proto` FSM, so every decrypt/parse/FSM path that could log is
/// exercised on the way to `CONNECTED`.
struct CorpusLink {
    corpus_json: String,
}

#[async_trait]
impl DeviceLink for CorpusLink {
    type Session = CorpusSession;

    async fn connect(&mut self) -> Result<Self::Session, LinkError> {
        let corpus: Value =
            serde_json::from_str(&self.corpus_json).expect("the golden corpus parses");
        let frames = corpus["frames"].as_array().expect("frames is a list");

        let rx: Vec<Vec<u8>> = frames
            .iter()
            .filter(|f| f["direction"] == "rx")
            .map(|f| hex_decode(f["wire_hex"].as_str().expect("wire_hex")))
            .collect();
        let pad_bytes: Vec<u8> = frames
            .iter()
            .filter(|f| f["direction"] == "tx")
            .flat_map(|f| pad_middle(f["pad_hex"].as_str().expect("pad_hex")))
            .collect();
        let nonce = frames
            .iter()
            .find(|f| f["direction"] == "tx" && f["resource"] == "/ci/authentication")
            .and_then(|f| f["json"]["data"][0]["nonce"].as_str())
            .expect("the corpus carries the /ci/authentication nonce")
            .to_owned();

        let keys = SessionKeys::from_base64(
            corpus["key"]["psk64"].as_str().expect("psk64"),
            corpus["key"]["iv64"].as_str().expect("iv64"),
        )
        .expect("the synthetic psk64/iv64 decode");

        let config = HandshakeConfig {
            identity: ProtoIdentity {
                name: corpus["client"]["device_name"]
                    .as_str()
                    .expect("device_name")
                    .to_owned(),
                id: corpus["client"]["device_id"]
                    .as_str()
                    .expect("device_id")
                    .to_owned(),
            },
        };

        let mut driver = Driver::new(
            FakeSocket::new(rx),
            AesSession::new(keys, Role::Client),
            DriverSources {
                clock: ZeroClock,
                nonce: RecordedNonce(nonce),
                pad: PadStream {
                    bytes: pad_bytes,
                    pos: 0,
                },
            },
            Handshake::new(config),
        );

        let outcome = driver
            .run_handshake()
            .map_err(|error| LinkError::Offline(error.to_string()))?;
        if outcome.state == State::Connected {
            Ok(CorpusSession)
        } else {
            Err(LinkError::Ended(format!(
                "handshake ended in {:?}",
                outcome.state
            )))
        }
    }
}

/// Decodes an even-length hex string. The corpus hex is a fixed, well-formed test input.
fn hex_decode(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hi =
                u8::try_from((pair[0] as char).to_digit(16).expect("hex digit")).expect("nibble");
            let lo =
                u8::try_from((pair[1] as char).to_digit(16).expect("hex digit")).expect("nibble");
            (hi << 4) | lo
        })
        .collect()
}

/// The pad's random middle: the bytes between the leading `0x00` and the trailing length byte,
/// exactly what `PadSource::fill_pad` supplies.
fn pad_middle(pad_hex: &str) -> Vec<u8> {
    let pad = hex_decode(pad_hex);
    pad.get(1..pad.len().saturating_sub(1))
        .expect("pad has a leading and trailing byte")
        .to_vec()
}

// =============================================================================================
// Fake broker — records every publish, subscription, and the connect-time will.
// =============================================================================================

#[derive(Debug, Default)]
struct FakeBroker {
    publishes: Vec<(String, Vec<u8>)>,
    subscriptions: Vec<String>,
    last_will: Option<LastWill>,
}

impl FakeBroker {
    fn with_will(will: LastWill) -> Self {
        Self {
            last_will: Some(will),
            ..Self::default()
        }
    }
}

#[async_trait]
impl MqttClient for FakeBroker {
    async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        _retain: bool,
    ) -> Result<(), MqttError> {
        self.publishes.push((topic.to_owned(), payload.to_vec()));
        Ok(())
    }
    async fn subscribe(&mut self, topic_filter: &str) -> Result<(), MqttError> {
        self.subscriptions.push(topic_filter.to_owned());
        Ok(())
    }
}

// =============================================================================================
// The captured emission surface, and the sweep over it.
// =============================================================================================

/// One named emission surface and every string it produced.
#[derive(Debug, Clone)]
struct Surface {
    source: &'static str,
    strings: Vec<String>,
}

/// Collects every emitted string from the driven session, grouped by surface.
fn captured_surfaces(bus: &TraceBus, broker: &FakeBroker) -> Vec<Surface> {
    // 1. tracing / trace-bus: target + message + every rendered field (key and value).
    let mut trace = Vec::new();
    for event in bus.recent() {
        trace.push(event.target);
        trace.push(event.message);
        for (key, value) in event.fields {
            trace.push(key);
            trace.push(value);
        }
    }

    // 2. every MQTT publish: topic and (UTF-8) payload, including every HA discovery config.
    let mut publishes = Vec::new();
    for (topic, payload) in &broker.publishes {
        publishes.push(topic.clone());
        if let Ok(text) = String::from_utf8(payload.clone()) {
            publishes.push(text);
        }
    }

    // 3. subscription filters. 4. the LWT / will.
    let subscriptions = broker.subscriptions.clone();
    let mut will = Vec::new();
    if let Some(w) = &broker.last_will {
        will.push(w.topic.clone());
        if let Ok(text) = String::from_utf8(w.payload.clone()) {
            will.push(text);
        }
    }

    let surfaces = vec![
        Surface {
            source: "tracing/trace-bus",
            strings: trace,
        },
        Surface {
            source: "mqtt-publish (topics + payloads + discovery)",
            strings: publishes,
        },
        Surface {
            source: "mqtt-subscribe",
            strings: subscriptions,
        },
        Surface {
            source: "mqtt-will (LWT)",
            strings: will,
        },
    ];

    // SSE is swept by `apps/server/tests/l10_sse_sweep.rs`; that crate depends on this one.

    surfaces
}

/// A value that must not appear on any surface. `value` is never rendered by the sweep.
#[derive(Debug, Clone)]
struct Needle {
    source: &'static str,
    value: String,
}

/// Every surface/needle hit, described by source and surface only, never by value.
fn find_leaks(surfaces: &[Surface], needles: &[Needle]) -> Vec<String> {
    let mut leaks = Vec::new();
    for surface in surfaces {
        for needle in needles {
            if surface
                .strings
                .iter()
                .any(|text| text.contains(&needle.value))
            {
                leaks.push(format!(
                    "a needle from `{}` leaked into surface `{}`",
                    needle.source, surface.source
                ));
            }
        }
    }
    leaks
}

// =============================================================================================
// Needle loading — all at runtime.
// =============================================================================================

/// Pushes `value` as a needle if it is long enough to be a genuine identifier (guards against a
/// short token causing spurious substring matches).
fn push_needle(needles: &mut Vec<Needle>, source: &'static str, value: &str) {
    if value.len() >= 6 {
        needles.push(Needle {
            source,
            value: value.to_owned(),
        });
    }
}

/// The synthetic corpus identifiers + psk/iv, read from the corpus at runtime.
fn corpus_needles(corpus_json: &str) -> Vec<Needle> {
    let corpus: Value = serde_json::from_str(corpus_json).expect("corpus parses");
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
        corpus["key"]["psk64"].as_str().unwrap_or(""),
    );
    push_needle(
        &mut needles,
        "corpus key",
        corpus["key"]["iv64"].as_str().unwrap_or(""),
    );
    needles
}

/// The scrubbed hob profile's own haId / serial / mac, via `expose_secret`.
fn hob_identity_needles(hob: &Profile) -> Vec<Needle> {
    let identity = hob.identity();
    let mut needles = Vec::new();
    push_needle(
        &mut needles,
        "hob profile identity",
        identity.ha_id.expose_secret(),
    );
    if let Some(serial) = identity.serial_number.as_ref() {
        push_needle(&mut needles, "hob profile identity", serial.expose_secret());
    }
    if let Some(mac) = identity.mac.as_ref() {
        push_needle(&mut needles, "hob profile identity", mac.expose_secret());
    }
    needles
}

/// Announces a skipped env-gated subset on the process's real stderr.
///
/// Not `eprintln!`: libtest captures it, so a passing test's skip would be invisible without
/// `--nocapture`. Writing to the `stderr` handle bypasses that capture.
fn announce(message: &str) {
    use std::io::Write as _;
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{message}");
    let _ = err.flush();
}

/// The real appliance identifiers from `secrets/`, if `KESSEL_RAW_PROFILES` points at them.
/// Returns `None`, with an announced skip, otherwise.
fn secrets_needles() -> Option<Vec<Needle>> {
    let dir = raw_profiles_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut needles = Vec::new();
    let mut found = 0usize;
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if path.extension().is_none_or(|ext| ext != "zip") {
            continue;
        }
        let Ok(profile) = Profile::from_zip_path(&path) else {
            continue;
        };
        needles.extend(hob_identity_needles_named(
            &profile,
            "secrets/ real identity",
        ));
        let creds = profile.credentials();
        push_needle(
            &mut needles,
            "secrets/ real key",
            creds.psk64.expose_secret(),
        );
        if let Some(iv) = creds.iv64.as_ref() {
            push_needle(&mut needles, "secrets/ real key", iv.expose_secret());
        }
        found += 1;
    }
    if found == 0 {
        announce(&format!(
            "SKIP full_session_emits_no_secret_values (secrets subset): no readable profile ZIPs \
             in {}",
            dir.display()
        ));
        return None;
    }
    Some(needles)
}

/// Like [`hob_identity_needles`] but with a caller-supplied source label.
fn hob_identity_needles_named(profile: &Profile, source: &'static str) -> Vec<Needle> {
    let identity = profile.identity();
    let mut needles = Vec::new();
    push_needle(&mut needles, source, identity.ha_id.expose_secret());
    if let Some(serial) = identity.serial_number.as_ref() {
        push_needle(&mut needles, source, serial.expose_secret());
    }
    if let Some(mac) = identity.mac.as_ref() {
        push_needle(&mut needles, source, mac.expose_secret());
    }
    needles
}

/// Resolves the `secrets/` directory from `KESSEL_RAW_PROFILES`: `1`/`true` →
/// `<workspace>/secrets`; any other non-empty value → that directory. Unset and empty are the same
/// case, and both are announced as a skip.
fn raw_profiles_dir() -> Option<PathBuf> {
    let value = std::env::var("KESSEL_RAW_PROFILES")
        .ok()
        .unwrap_or_default();
    if value.is_empty() {
        announce(
            "SKIP full_session_emits_no_secret_values (secrets subset): KESSEL_RAW_PROFILES is \
             not set, so secrets/-derived needles were not swept (synthetic + profile identity needles were)",
        );
        return None;
    }
    if value == "1" || value == "true" {
        Some(workspace_root().join("secrets"))
    } else {
        Some(PathBuf::from(value))
    }
}

// =============================================================================================
// The test.
// =============================================================================================

/// A synthetic value injected into a throwaway surface copy to prove the sweep is non-vacuous.
const SENTINEL: &str = "L10-SWEEP-SENTINEL-VALUE";

/// Drives the full session (supervisor over the corpus handshake, plus the MQTT bridge) and
/// returns the captured surfaces plus the bus (kept alive for the trace surface).
async fn drive_session(bus: &Arc<TraceBus>) -> FakeBroker {
    // 1. The supervisor drives one full connect→steady→reconnect cycle over the REAL corpus
    // handshake, emitting connection-state transitions to the trace bus.
    let shadow = Arc::new(Mutex::new(Shadow::new()));
    let config = DeviceConfig {
        role: DeviceRole::Cooktop,
        backoff: Backoff::new(std::time::Duration::ZERO, std::time::Duration::ZERO, 1.0),
        rng_seed: 1,
        max_attempts: Some(1),
        shutdown: None,
    };
    let link = CorpusLink {
        corpus_json: std::fs::read_to_string(corpus_path()).expect("corpus readable"),
    };
    run_device(link, config, Arc::clone(bus), shadow).await;

    // 2. The MQTT bridge announces the scrubbed hob profile over a fake broker: discovery
    // (full), raw state topics, `…/set` subscriptions, availability, every connection_state, will.
    let hob = Profile::from_zip_bytes(&std::fs::read(hob_zip_path()).expect("hob zip readable"))
        .expect("hob profile parses");
    let model = hob.model();
    let device = DeviceId::new("kitchen-hob");
    let mapping = DeviceMapping::build(&device, model, &Excludes::none());
    let bridge = DeviceBridge::new("kessel", device);

    let mut broker = FakeBroker::with_will(bridge.last_will());
    bridge
        .announce(
            &mut broker,
            &mapping,
            &Announcement::bare(DiscoveryMode::Full),
        )
        .await
        .expect("announce");
    bridge
        .publish_availability(&mut broker, true)
        .await
        .expect("availability");
    for state in [
        ConnectionState::Connecting,
        ConnectionState::Handshake,
        ConnectionState::Connected,
        ConnectionState::Reconnecting,
        ConnectionState::Offline,
    ] {
        bridge
            .publish_connection_state(&mut broker, state)
            .await
            .expect("connection_state");
    }
    broker
}

/// With `KESSEL_RAW_PROFILES` unset, the `secrets/` skip is announced.
///
/// The announcement goes to the real stderr (see [`announce`]), so this re-runs the test binary as
/// a child with the variable removed and asserts over the child's stderr.
#[test]
fn an_unset_raw_profiles_gate_announces_its_skip() {
    let exe = std::env::current_exe().expect("this test binary's own path");
    let output = std::process::Command::new(exe)
        .args(["--exact", "full_session_emits_no_secret_values"])
        .env_remove("KESSEL_RAW_PROFILES")
        .output()
        .expect("re-running this test binary as a child");
    assert!(
        output.status.success(),
        "the child run of the sweep itself must pass; it exited {:?}",
        output.status.code()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "SKIP full_session_emits_no_secret_values (secrets subset): KESSEL_RAW_PROFILES is not \
             set"
        ),
        "with KESSEL_RAW_PROFILES unset the sweep must ANNOUNCE that it skipped the secrets/ \
         subset (a skip must never be silent), but no such line reached stderr. Child stderr \
         was: {stderr:?}"
    );
}

#[test]
fn full_session_emits_no_secret_values() {
    // A capturing subscriber at TRACE, sharing the supervisor's bus, so both `tracing` macro events
    // (hc-proto) and the supervisor's direct emits land in one sink. A current-thread runtime keeps
    // every future on this thread, so the thread-local subscriber sees the whole driven stack.
    let bus = Arc::new(TraceBus::new(1_000_000));
    let subscriber = tracing_subscriber::registry().with(TraceBusLayer::new(Arc::clone(&bus)));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");

    let broker = tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(async { drive_session(&bus).await })
    });

    let surfaces = captured_surfaces(&bus, &broker);

    // The session really ran through the real stack: the supervisor reached CONNECTED (proving the
    // corpus handshake drove to completion), and there is a non-empty surface to sweep.
    assert!(
        surfaces
            .iter()
            .any(|s| s.source.starts_with("tracing") && s.strings.iter().any(|t| t == "connected")),
        "the supervisor must have emitted a CONNECTED transition (the corpus handshake completed)"
    );
    let total: usize = surfaces.iter().map(|s| s.strings.len()).sum();
    assert!(
        total > 20,
        "the swept surface must be substantial, got {total} strings"
    );

    // Assemble the needle set, every needle loaded at runtime.
    let corpus_json = std::fs::read_to_string(corpus_path()).expect("corpus readable");
    let hob = Profile::from_zip_bytes(&std::fs::read(hob_zip_path()).expect("hob zip"))
        .expect("hob parses");
    let mut needles = corpus_needles(&corpus_json);
    needles.extend(hob_identity_needles(&hob));
    let secrets_swept = secrets_needles().is_some_and(|secret_needles| {
        needles.extend(secret_needles);
        true
    });
    assert!(
        needles.len() >= 15,
        "the needle set must be populated (synthetic + profile identity needles are always available), got {}",
        needles.len()
    );

    // NON-VACUITY: prove the capture + sweep can actually catch a leak. Inject a sentinel into a
    // THROWAWAY copy of a real surface, add it as a needle, and confirm the sweep flags it — then
    // confirm the sentinel exists ONLY in the throwaway, never in the real capture.
    let mut throwaway = surfaces.clone();
    throwaway
        .iter_mut()
        .find(|s| s.source.starts_with("mqtt-publish"))
        .expect("the mqtt-publish surface exists")
        .strings
        .push(format!("kessel/kitchen-hob/state {SENTINEL}"));
    let sentinel_needle = Needle {
        source: "injected sentinel",
        value: SENTINEL.to_owned(),
    };
    assert!(
        !find_leaks(&throwaway, std::slice::from_ref(&sentinel_needle)).is_empty(),
        "non-vacuity: the sweep MUST detect an injected sentinel — else the absence check is \
         worthless"
    );
    assert!(
        find_leaks(&surfaces, std::slice::from_ref(&sentinel_needle)).is_empty(),
        "the sentinel must exist only in the throwaway copy, never in the real capture"
    );

    // THE SWEEP: zero occurrences of any needle across the whole captured surface.
    let leaks = find_leaks(&surfaces, &needles);
    assert!(
        leaks.is_empty(),
        "an appliance identifier or key leaked into emitted surfaces ({} leak(s)): {:?}",
        leaks.len(),
        leaks
    );

    if !secrets_swept {
        announce(&format!(
            "NOTE full_session_emits_no_secret_values: swept {} needles (synthetic corpus, hob \
             profile identity). secrets/ real identifiers NOT swept — set \
             KESSEL_RAW_PROFILES to include them.",
            needles.len()
        ));
    }
}
