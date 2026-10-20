//! The live write: a no-op write to a real dishwasher and fridge.
//!
//! # Device safety
//!
//! This test contacts the dishwasher and the fridge only. [`WRITABLE_ROLES`] is checked before a
//! socket is opened, so an address set for the hob or the oven is never dialled.
//!
//! The write is a no-op by construction: the value written is the value the appliance just reported
//! in its own NOTIFY stream. Nothing starts, stops, pauses or schedules a program; nothing opens or
//! releases a lock; no temperature setpoint is touched.
//!
//! If the appliance refuses the write, that is the result: the frame, the response and the refusal
//! are printed and the test stops. It never iterates key orders, uids or body shapes against live
//! hardware.
//!
//! # Gates
//!
//! 1. `#[ignore]`: `just live` passes `--ignored`, and nothing else runs this;
//! 2. `KESSEL_LIVE_PROFILE_DIR` pointing at a directory of appliance profile ZIPs (the gitignored
//!    `secrets/` tree), supplied through the environment and never committed.
//!
//! Every skip prints why.
//!
//! # Serialisation
//!
//! These appliances accept a second concurrent session and then both return garbage, so one test
//! function walks the two appliances sequentially, each connection fully dropped before the next is
//! dialled. Pass `--test-threads=1` anyway.
//!
//! # Per appliance
//!
//! 1. connect and handshake through the real device task (`LiveDeviceLink`), which installs the
//!    device's guarded writer exactly as production does;
//! 2. pump steady state briefly so the appliance's NOTIFYs arrive;
//! 3. pick a uid the appliance's own `DeviceDescription` declares writable and that appears in the
//!    NOTIFY stream with a current value. If none qualifies, nothing is written;
//! 4. write that value back through the real `POST /api/devices/{slug}/write` route;
//! 5. pump again and read the value back.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions; indexing a JSON response panics loudly \
                  on a missing key"
    )
)]
#![allow(
    clippy::print_stderr,
    reason = "a live run is read off the terminal: the per-appliance report and every explicit skip \
              reason must be visible under `--nocapture`; no `tracing` subscriber is installed in a \
              test binary"
)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hc_core::device::DeviceRole;
use hc_core::supervisor::DeviceLink as _;
use hc_profile::Profile;
use server::auth::{ApiToken, TOKEN_HEADER};
use server::live_transport::{
    DeviceTaskConfig, LiveDeviceLink, known_uids, select_transport, updates_from_notifications,
};
use server::onboarding::{role_for_type, target_for};
use server::state::{AppState, DeviceEntry};

/// The directory holding the appliance profile ZIPs. Never a path inside the repo.
const PROFILE_DIR_ENV: &str = "KESSEL_LIVE_PROFILE_DIR";

/// The token this test authorises its own requests with. Not a secret and not a real one.
const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// The only two roles this test will contact.
const WRITABLE_ROLES: [&str; 2] = ["dishwasher", "fridge"];

/// How long steady state is pumped so the appliance's NOTIFYs arrive.
const PUMP: Duration = Duration::from_secs(12);

/// How long the appliance is left alone between connections, so the previous session is fully torn
/// down before the next dial.
const SETTLE: Duration = Duration::from_secs(2);

/// The address to dial for `slug`, from `KESSEL_LIVE_HOST_<SLUG>`, or `None`, which is a refusal
/// to dial, not a default.
fn host_for(slug: &str) -> Option<String> {
    std::env::var(format!("KESSEL_LIVE_HOST_{}", slug.to_ascii_uppercase())).ok()
}

/// The `.zip` profiles in `dir`, in a deterministic order so a run is reproducible.
fn profiles_in(dir: &str) -> Vec<PathBuf> {
    let mut zips: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("the profile directory is readable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "zip"))
        .collect();
    zips.sort();
    zips
}

/// One appliance under test.
struct Appliance<'a> {
    profile: &'a Profile,
    role: &'a DeviceRole,
    slug: &'a str,
}

/// How many messages of each kind had arrived at a point in time — the before/after oracle for
/// "did the appliance answer the write".
#[derive(Debug, Clone, Copy)]
struct Counts {
    ro_values: usize,
    notifications: usize,
}

/// A write-back target: a uid the appliance declares writable that is currently reporting a value.
#[derive(Debug, Clone)]
struct Target {
    uid: u64,
    feature_key: String,
    access: String,
    current: serde_json::Value,
}

#[tokio::test]
#[ignore = "CONTACTS AND WRITES TO real appliances (dishwasher and fridge only); needs \
            KESSEL_LIVE_PROFILE_DIR. There is no gate left: running this performs a real write."]
async fn live_no_op_write_to_the_dishwasher_and_the_fridge() {
    let Ok(dir) = std::env::var(PROFILE_DIR_ENV) else {
        eprintln!(
            "SKIP live_no_op_write_to_the_dishwasher_and_the_fridge: {PROFILE_DIR_ENV} is unset, so \
             no profile was read and nothing was contacted."
        );
        return;
    };

    // Running this test at all is the deliberate act; `WRITABLE_ROLES` below is this test's own
    // scope, not a Kessel policy.
    eprintln!(
        "\n=== LIVE WRITE. Dishwasher and fridge only — this test's own scope, not a gate.\n"
    );

    for zip in profiles_in(&dir) {
        // Never `zip.display()`: a real profile's filename carries the serial or haId, which must
        // never be logged. Everything below is attributed by role class instead.
        let bytes = std::fs::read(&zip).expect("the profile ZIP is readable");
        let profile = Profile::from_zip_bytes(&bytes).expect("the profile ZIP parses");
        let role = role_for_type(&profile.identity().device_type);
        let slug = role.label().to_owned();

        // Nothing downstream refuses a hob or oven write, so this check is what keeps the test to
        // the dishwasher and the fridge.
        if !WRITABLE_ROLES.contains(&slug.as_str()) {
            eprintln!(
                "SKIP {slug}: not one of the two authorised roles ({WRITABLE_ROLES:?}). No socket \
                 was opened to it."
            );
            continue;
        }
        let Some(host) = host_for(&slug) else {
            eprintln!("SKIP {slug}: no LAN address is known for it; contacting nothing.");
            continue;
        };

        exercise(
            &Appliance {
                profile: &profile,
                role: &role,
                slug: &slug,
            },
            host,
        )
        .await;
        tokio::time::sleep(SETTLE).await;
    }
}

/// Builds the device link for one appliance. **Constructing it opens no socket.**
fn device_link(
    appliance: &Appliance<'_>,
    host: String,
    uids: &std::collections::BTreeSet<u64>,
    state: &AppState,
) -> LiveDeviceLink {
    let Appliance {
        profile,
        role,
        slug,
    } = appliance;
    // The manual LAN address, not the manifest-derived host, which is only the fallback. `None`: the
    // fleet is on the transports' default ports.
    let mut target = target_for(profile, None);
    target.host = host;
    eprintln!("--- {slug} ({:?}): connecting", select_transport(&target));
    LiveDeviceLink::new(DeviceTaskConfig {
        slug: (*slug).to_owned(),
        role: (*role).clone(),
        target,
        known_uids: uids.clone(),
        writers: Arc::clone(state.writers()),
        // This test drives `pump` rather than `run_steady_state`, so no shadow is installed or read.
        shadows: Arc::clone(state.shadows()),
        // No broker in a live test: nothing is published.
        events: None,
        // Real objects, not stubs, though this test reads `notify_stream` and never subscribes.
        model: std::sync::Arc::new(profile.model().clone()),
        bus: Arc::clone(state.bus()),
        // The task records its connection transitions on the entry this test registered.
        devices: Arc::clone(state.devices()),
    })
}

/// Connects to one appliance, reads, and writes one value back.
async fn exercise(appliance: &Appliance<'_>, host: String) {
    let Appliance {
        profile,
        role,
        slug,
    } = appliance;
    let state = AppState::new(common::bus()).with_token(ApiToken::from_string(TEST_TOKEN));
    state.devices().insert(
        *slug,
        DeviceEntry::from_profile(slug, (*role).clone(), profile),
    );
    let entry = state.devices().entry(slug).expect("just registered");
    let uids = known_uids(&entry.model);
    let mut link = device_link(appliance, host, &uids, &state);

    let mut held = match link.connect().await {
        Ok(held) => held,
        Err(error) => {
            // An unreachable device is a blocked outcome, never a silent skip.
            eprintln!(
                "!!! {slug}: COULD NOT CONNECT — {error}. This is a BLOCKED outcome for this \
                 appliance, not a pass. Nothing was written."
            );
            return;
        }
    };
    eprintln!("--- {slug}: CONNECTED; its guarded writer is installed");
    assert!(
        state.writers().writer(slug).is_some(),
        "{slug}: a connected device must have its guarded writer installed"
    );

    held.pump(PUMP).await;
    let updates = current_values(&held, &uids);
    eprintln!(
        "--- {slug}: {} /ro snapshot message(s), {} NOTIFY message(s), {} decoded value update(s)",
        held.ro_values().len(),
        held.notify_stream().len(),
        updates.len()
    );

    report_candidate_sets(&entry, slug, &updates);

    let Some(target) = pick_target(&entry, &updates) else {
        eprintln!(
            "!!! {slug}: NO WRITE PERFORMED. No uid is both declared writable by the appliance's own \
             DeviceDescription and currently reporting a value in its NOTIFY stream, so there is no \
             value to write back unchanged. Writing anything else would be inventing a value, which \
             is not allowed. This is a documented refusal, not a failure."
        );
        return;
    };
    eprintln!(
        "--- {slug}: write-back target uid={} key={} access={} current={}",
        target.uid, target.feature_key, target.access, target.current
    );

    // Snapshot the message counts before the write so the read-back can say whether the appliance
    // answered. The write is fire-and-forget (`Sender::send` bypasses the correlator), so HTTP 200
    // only means the frame was transmitted; an uncorrelated RESPONSE to our `/ro/values` msgID, which
    // lands in the retained `/ro` set, is the only in-band evidence of acceptance.
    let before = Counts {
        ro_values: held.ro_values().len(),
        notifications: held.notify_stream().len(),
    };
    write_back(&state, &entry, slug, &target).await;
    read_back(&mut held, slug, &target, &uids, before).await;
}

/// Pumps the connection again and reports what the appliance now says the uid holds.
///
/// A no-op write must leave it unchanged. An absent NOTIFY is reported but not asserted on: an
/// appliance need not re-announce a value that did not change.
async fn read_back(
    held: &mut server::live_transport::LiveDeviceSession<
        Box<dyn server::live_transport::HeldSession>,
    >,
    slug: &str,
    target: &Target,
    uids: &std::collections::BTreeSet<u64>,
    before: Counts,
) {
    held.pump(PUMP).await;
    // The evidence that separates "transmitted" from "accepted by the appliance".
    eprintln!(
        "--- {slug}: after the write — /ro messages {} -> {}, NOTIFY {} -> {}. The connection is \
         still alive (this pump ran on it). A new /ro message here would be the appliance ANSWERING \
         the write; none means the write was transmitted and not answered in band, which is what a \
         fire-and-forget POST looks like either way.",
        before.ro_values,
        held.ro_values().len(),
        before.notifications,
        held.notify_stream().len()
    );
    // An absent `code` means the appliance acknowledged the write; a code is a refusal. Only the
    // envelope is printed, never the body.
    for message in held.ro_values().iter().skip(before.ro_values) {
        eprintln!(
            "--- {slug}: THE APPLIANCE ANSWERED THE WRITE: resource={:?} action={:?} code={:?} \
             data_items={}",
            message.resource,
            message.action,
            message.code,
            message.data.as_ref().map_or(0, Vec::len)
        );
    }
    let after = current_values(held, uids);
    let readback = after
        .iter()
        .rev()
        .find(|update| update.uid == target.uid)
        .map(|update| update.value.clone());
    eprintln!(
        "--- {slug}: read-back uid={} before={} after={}",
        target.uid,
        target.current,
        readback.as_ref().map_or_else(
            || "<no NOTIFY for this uid after the write>".to_owned(),
            std::string::ToString::to_string
        )
    );
    if let Some(value) = readback {
        assert_eq!(
            value, target.current,
            "{slug}: the no-op write must leave the value unchanged — this is the whole point of \
             writing back what the appliance reported"
        );
    }
}

/// Prints the two sets whose intersection is the write-back candidate.
///
/// This lets a reader tell a genuinely empty intersection from a wrong filter. Uids, feature keys
/// and values are capability metadata and appliance state, not identifiers or key material.
fn report_candidate_sets(entry: &DeviceEntry, slug: &str, updates: &[hc_proto::ValueUpdate]) {
    let declared: Vec<String> = entry
        .entity_descriptors(None)
        .into_iter()
        .filter(|descriptor| descriptor.writable)
        .map(|descriptor| {
            format!(
                "{}:{}:{}",
                descriptor.uid,
                descriptor.access.as_deref().unwrap_or("<none>"),
                descriptor.feature_key.as_deref().unwrap_or("<no key>")
            )
        })
        .collect();
    let descriptors = entry.entity_descriptors(None);
    let reported: Vec<String> = updates
        .iter()
        .map(|update| {
            descriptors
                .iter()
                .find(|candidate| u64::from(candidate.uid) == update.uid)
                .map_or_else(
                    || format!("{}:<not in the mapping>", update.uid),
                    |descriptor| {
                        format!(
                            "{}:{}:writable={}:{}",
                            update.uid,
                            descriptor.access.as_deref().unwrap_or("<none>"),
                            descriptor.writable,
                            descriptor.feature_key.as_deref().unwrap_or("<no key>")
                        )
                    },
                )
        })
        .collect();
    eprintln!("--- {slug}: writable declared (uid:access:key) = {declared:?}");
    eprintln!("--- {slug}: reporting a value (uid:access:writable:key) = {reported:?}");
}

/// Every value the appliance currently reports: the `/ro` snapshot, then the NOTIFY stream.
///
/// The order is the precedence: callers take the last entry for a uid, so a NOTIFY after the
/// handshake wins. Without the snapshot a standby appliance reports nothing at all.
fn current_values(
    held: &server::live_transport::LiveDeviceSession<Box<dyn server::live_transport::HeldSession>>,
    uids: &std::collections::BTreeSet<u64>,
) -> Vec<hc_proto::ValueUpdate> {
    let mut values = updates_from_notifications(held.ro_values(), uids);
    values.extend(updates_from_notifications(held.notify_stream(), uids));
    values
}

/// Sends the no-op write through the real HTTP route and reports the outcome.
async fn write_back(state: &AppState, entry: &DeviceEntry, slug: &str, target: &Target) {
    // Through the router, so the whole production path (token gate, feature resolution,
    // `build_write_command`, registry lookup, guard) carries the write.
    let submitted = value_as_submitted(entry, target);
    let body = serde_json::json!({
        "feature_key": target.feature_key,
        "value": submitted,
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/devices/{slug}/write"))
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(body.to_string()))
        .expect("a well-formed request");

    eprintln!(
        "--- {slug}: WRITING uid={} key={} reported={} submitted-as={submitted:?} (the value the \
         appliance itself reported; see value_as_submitted for why the two spellings differ)",
        target.uid, target.feature_key, target.current
    );
    let reply = common::send(server::api::app(state.clone()), request).await;
    eprintln!("--- {slug}: HTTP {} {}", reply.status, reply.body);
    if reply.status != StatusCode::OK {
        // A refusal is a finding, not something to retry with a different shape.
        eprintln!(
            "!!! {slug}: THE WRITE WAS REFUSED. Recorded above verbatim. No variant was tried: \
             iterating key orders or uids against live hardware is not allowed."
        );
    }
}

/// The value as the SPA would submit it: the API takes a `String`.
///
/// The appliance reports an enumeration as its wire ordinal (`0`). `hc_core::bridge::enum_wire_value`
/// accepts either a declared member label or the ordinal, but this translates the ordinal to the
/// label because that is what the SPA submits (its `select` renders member names).
fn value_as_submitted(entry: &DeviceEntry, target: &Target) -> String {
    let ordinal_label = entry
        .model
        .entity_by_key(&target.feature_key)
        .and_then(|entity| {
            let enum_type = entry.model.enum_type_of(entity)?;
            let ordinal = target.current.as_i64()?;
            enum_type.member(ordinal).map(str::to_owned)
        });
    match (ordinal_label, &target.current) {
        (Some(label), _) => label,
        (None, serde_json::Value::String(text)) => text.clone(),
        (None, other) => other.to_string(),
    }
}

/// The features this test is willing to write, most preferred first.
///
/// A named list rather than "the first writable one", which on the real dishwasher is
/// `BSH.Common.Setting.AllowBackendConnection` (cloud connectivity). All three are clock display
/// settings: if a malformed frame applied a different value, the worst outcome is a clock showing
/// 12-hour instead of 24-hour time.
///
/// Deliberately not on this list: `AllowBackendConnection`, `RemoteControlLevel`, `PowerState`,
/// anything `Program`-, `Light`-, `Setpoint`- or `Temperature`-named, and every `writeOnly` command.
const PREFERRED_TARGETS: [&str; 3] = [
    "BSH.Common.Setting.Time.DisplayMode",
    "BSH.Common.Setting.TimeFormat",
    "BSH.Common.Setting.Timezone",
];

/// Picks the write-back target: on [`PREFERRED_TARGETS`], declared `readWrite` by the appliance, and
/// currently reporting a value.
///
/// `access` comes from the appliance's own `DeviceDescription` and the value from its own `/ro`
/// snapshot or NOTIFY; the value written is always the one just read. `None` means no write.
///
/// Commands (`writeOnly`) report no value, so they can never be selected.
fn pick_target(entry: &DeviceEntry, updates: &[hc_proto::ValueUpdate]) -> Option<Target> {
    let descriptors = entry.entity_descriptors(None);
    PREFERRED_TARGETS.iter().find_map(|preferred| {
        let descriptor = descriptors.iter().find(|candidate| {
            candidate.feature_key.as_deref() == Some(*preferred)
                && candidate.writable
                && candidate.access.as_deref() == Some("readWrite")
        })?;
        // The last update for the uid wins: `current_values` puts the /ro snapshot first.
        let current = updates
            .iter()
            .rev()
            .find(|update| update.uid == u64::from(descriptor.uid))?
            .value
            .clone();
        Some(Target {
            uid: u64::from(descriptor.uid),
            feature_key: (*preferred).to_owned(),
            access: descriptor.access.clone().unwrap_or_default(),
            current,
        })
    })
}
