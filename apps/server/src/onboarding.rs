//! Onboarding: paste a profile ZIP, parse it, connection-test it, persist it.
//!
//! On connect failure the operator is offered a manual-IP retry.
//!
//! The connect + handshake sits behind [`ConnectionTester`]. The default, [`NoLiveTransport`],
//! refuses; `main.rs` installs [`crate::live_transport::LiveConnectionTester`]. So a test binary
//! contacts no appliance unless it explicitly asks to.
//!
//! Nothing in this module writes to an appliance: the connection test connects, handshakes, reads
//! and disconnects. The write path is [`crate::write::WriterRegistry`], which only the device task
//! populates, and only after CONNECTED.
//!
//! The suggested slug comes from the appliance class (`hob` → `cooktop`, plus a numeric suffix if
//! taken), never from its identity: appliance identifiers must never reach a topic, an SSE frame or
//! a log, and a slug appears in every topic and route, so it should be stable and readable.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use hc_core::device::{ConnectionState, DeviceRole};
use hc_crypto::Secret;
use hc_profile::{ConnectionType, Profile, ProfileError};

use crate::state::{AppState, DeviceEntry};
use crate::types::{CommitRequest, ConnectionTest, HostSource, OnboardingResult, ProfilePreview};

/// How long a connection test may take before it is a failure.
///
/// A real connect + handshake takes about 20 s, and `Session` uses a 20 s per-request deadline;
/// this is the whole-test budget, so it is larger.
pub const CONNECTION_TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What a connection test is asked to dial.
///
/// `aes_ws::connect` needs [`hc_crypto::SessionKeys`] derived from `psk64` + `iv64`, and
/// `tls_psk::connect` needs the decoded `psk64`. Both are [`Secret`]s, so the derived `Debug` redacts
/// them. They are copied out of `hc_profile::Credentials` because that type is `#[non_exhaustive]`
/// and tests need to construct a target.
#[derive(Debug, Clone)]
pub struct ConnectionTarget {
    /// The host: the manifest-derived default, or the operator's manual override.
    pub host: String,
    /// Which transport the manifest says this appliance speaks (AES over `ws`, or TLS-PSK).
    pub connection_type: ConnectionType,
    /// The appliance `key`, base64url. The AES KDF's input, or the TLS-PSK callback's.
    pub psk64: Secret<String>,
    /// The AES `iv`, base64url. `Some` exactly on the AES path, which is how
    /// `crate::live_transport::select_transport` picks the transport.
    pub iv64: Option<Secret<String>>,
    /// The operator's port override; `None` is the transport's default, 80 on AES and 443 on
    /// TLS-PSK (`hc_proto::Transport::default_port`). Needed for an appliance behind a port-forward.
    pub port: Option<u16>,
}

/// The connect + handshake attempt, behind a seam.
#[async_trait]
pub trait ConnectionTester: fmt::Debug + Send + Sync {
    /// Attempts one connect + handshake against `target`.
    ///
    /// # Errors
    ///
    /// An operator-facing reason. It must not carry an identifier: the reason reaches the UI and the
    /// trace bus.
    async fn test(&self, target: &ConnectionTarget) -> Result<(), String>;
}

/// The refusing tester, and the **default** one: it never opens a socket.
///
/// Not a stub that returns `Ok`: that would report a device as reachable without anything having
/// been dialled. `main.rs` always installs [`crate::live_transport::LiveConnectionTester`], so only
/// test binaries reach this type.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLiveTransport;

#[async_trait]
impl ConnectionTester for NoLiveTransport {
    async fn test(&self, _target: &ConnectionTarget) -> Result<(), String> {
        Err(
            // Names no variable: there is no switch to flip.
            "this build has no live connection tester installed, so no connection was attempted. \
             That is a defect rather than a setting: the daemon installs the real one at startup"
                .to_owned(),
        )
    }
}

/// Starting a newly onboarded device's long-lived task, behind a seam.
///
/// `spawn_device_tasks` runs once at startup, so a device onboarded later needs its task started
/// here. `AppState::new` installs the non-launching default below, like [`NoLiveTransport`], so a
/// test that onboards a fixture dials nothing; only `main.rs` installs
/// [`crate::live_transport::LiveDeviceLauncher`].
pub trait DeviceLauncher: fmt::Debug + Send + Sync {
    /// Starts `slug`'s device task, returning whether one was started.
    ///
    /// `false` means no task was started, which covers both "this build cannot start one" (the default
    /// below) and "this device already has one" — a caller must not read it as a failure.
    fn launch(&self, state: &AppState, slug: &str) -> bool;
}

/// The launcher that starts nothing, and the **default** one: it opens no socket.
///
/// Every test binary gets this one. The live counterpart is
/// [`crate::live_transport::LiveDeviceLauncher`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDeviceTasks;

impl DeviceLauncher for NoDeviceTasks {
    fn launch(&self, _state: &AppState, slug: &str) -> bool {
        tracing::debug!(
            device = %slug,
            "no device-task launcher is installed, so this device gets no task until a restart"
        );
        false
    }
}

/// Everything onboarding can refuse.
#[derive(Debug, thiserror::Error)]
pub enum OnboardingError {
    /// The posted bytes are not a profile ZIP this parser understands.
    ///
    /// The `ProfileError` is rendered, not held; its `Display` already redacts identifier-bearing
    /// member filenames.
    #[error("the profile could not be parsed: {0}")]
    Parse(String),
    /// No device is registered under that slug.
    #[error("no such device")]
    UnknownDevice,
    /// The operator's chosen slug is not usable as a device name.
    ///
    /// The message names the allowed characters.
    #[error("{0}")]
    InvalidSlug(String),
    /// The chosen slug belongs to a different appliance.
    ///
    /// The same slug for the same appliance is the upsert and is not an error.
    #[error(
        "another appliance is already onboarded as `{0}`; choose a different name, or remove that \
         device first"
    )]
    SlugTaken(String),
    /// The device has a running task, so it cannot be renamed or removed right now.
    ///
    /// A device task is detached and cannot be stopped (see `release_previous_slug`), so the
    /// instruction is to restart the daemon.
    #[error(
        "`{0}` is currently connected. Kessel cannot stop a device's connection without restarting, \
         and leaving it running would hold a session against an appliance you had removed. \
         Restart Kessel, then try again"
    )]
    DeviceBusy(String),
    /// The device was onboarded but could not be written to disk.
    ///
    /// Reported rather than swallowed: the device works now but will vanish on the next restart.
    #[error(
        "the device was onboarded but could not be saved, so it will not survive a restart: {0}"
    )]
    Storage(String),
}

impl From<ProfileError> for OnboardingError {
    fn from(error: ProfileError) -> Self {
        Self::Parse(error.to_string())
    }
}

/// The appliance role for a manifest `type`, defaulting to [`DeviceRole::Other`].
///
/// The role is a label and a discovery input, not a permission. Writability comes from the
/// appliance's own `DeviceDescription`: the mapping produces a `…/set` topic only for a feature the
/// firmware declares writable.
#[must_use]
pub fn role_for_type(device_type: &str) -> DeviceRole {
    match device_type.to_ascii_lowercase().as_str() {
        "hob" | "cooktop" => DeviceRole::Cooktop,
        "oven" | "cookprocessor" => DeviceRole::Oven,
        "dishwasher" => DeviceRole::Dishwasher,
        "fridgefreezer" | "fridge" | "freezer" | "refrigerator" => DeviceRole::Fridge,
        "washer" | "washingmachine" => DeviceRole::Washer,
        "dryer" => DeviceRole::Dryer,
        other => DeviceRole::Other(other.to_owned()),
    }
}

/// A slug for `role` that no registered device is using: the role label, then `-2`, `-3`, …
pub fn unique_slug(role: &DeviceRole, taken: impl Fn(&str) -> bool) -> String {
    let base = role.label().to_owned();
    if !taken(&base) {
        return base;
    }
    // Two of the same appliance class is normal (two ovens); a thousand is not, so the loop is
    // bounded and falls back rather than spinning.
    for suffix in 2..1000_u32 {
        let candidate = format!("{base}-{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{base}-overflow")
}

/// The target a profile should be dialled at: the manual host if set, else the derived one.
///
/// `port` is the operator's override, carried beside the profile. Both the onboarding connection
/// test and the device task build their target here, so a stored port applies to reconnects too.
#[must_use]
pub fn target_for(profile: &Profile, port: Option<u16>) -> ConnectionTarget {
    let credentials = profile.credentials();
    ConnectionTarget {
        host: profile.host(),
        connection_type: profile.identity().connection_type.clone(),
        psk64: credentials.psk64.clone(),
        iv64: credentials.iv64.clone(),
        port,
    }
}

/// The largest profile ZIP the API accepts, matching `hc_profile::MAX_MEMBER_BYTES`.
///
/// The parser already bounds each member; this bounds the upload, so a 2 GB POST is refused before
/// it is buffered.
pub const MAX_PROFILE_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Phase 1: parses a posted profile ZIP and reports what it says.
///
/// Registers nothing, persists nothing, opens no socket. The UI uses it to show the operator what the
/// archive contains and ask where the appliance is and what to call it before dialling.
///
/// See [`ProfilePreview`] for why the reply carries no host field.
///
/// # Errors
///
/// [`OnboardingError::Parse`] if the bytes are not a profile this parser understands.
pub fn preview(state: &AppState, zip: &[u8]) -> Result<ProfilePreview, OnboardingError> {
    let profile = Profile::from_zip_bytes(zip)?;
    let role = role_for_type(&profile.identity().device_type);
    let fingerprint = profile.fingerprint();
    let existing_slug = state.devices().slug_for_fingerprint(&fingerprint);

    // An appliance already onboarded defaults to updating itself rather than becoming `cooktop-2`.
    let suggested_slug = existing_slug
        .clone()
        .unwrap_or_else(|| unique_slug(&role, |candidate| state.devices().contains(candidate)));

    let target = target_for(&profile, None);
    let (transport, default_port) = match crate::live_transport::select_transport(&target) {
        crate::live_transport::WireTransport::Aes => {
            ("aes", hc_proto::Transport::Aes.default_port())
        }
        crate::live_transport::WireTransport::TlsPsk => {
            ("tls_psk", hc_proto::Transport::TlsPsk.default_port())
        }
    };

    tracing::info!(
        role = role.label(),
        features = profile.model().entities().len(),
        skipped = profile.skipped().len(),
        transport,
        update = existing_slug.is_some(),
        "previewed a profile ZIP; nothing was dialled or stored"
    );

    Ok(ProfilePreview {
        role: role.label().to_owned(),
        suggested_slug,
        existing_slug,
        entity_count: count(profile.model().entities().len()),
        skipped_entity_count: count(profile.skipped().len()),
        transport: transport.to_owned(),
        default_port,
    })
}

/// Phase 2: registers the device on the operator's confirmed terms, persists it, dials it, and
/// starts its task.
///
/// The profile is persisted before the dial, so an appliance that is merely powered off keeps its
/// upload and the address can be corrected afterwards.
///
/// Identity is [`Profile::fingerprint`], not the slug, so the three cases are:
///
/// * **same appliance, same slug** — update in place;
/// * **same appliance, new slug** — rename, releasing the old one;
/// * **different appliance, occupied slug** — [`OnboardingError::SlugTaken`]. Overwriting would
///   silently discard the first appliance's profile.
///
/// # Errors
///
/// [`OnboardingError::Parse`], [`OnboardingError::InvalidSlug`], [`OnboardingError::SlugTaken`],
/// [`OnboardingError::DeviceBusy`] (a rename of a connected device), [`OnboardingError::Storage`], or
/// [`OnboardingError::UnknownDevice`] if the device vanished from the registry mid-request.
pub async fn commit(
    state: &AppState,
    zip: &[u8],
    request: &CommitRequest,
) -> Result<OnboardingResult, OnboardingError> {
    let profile = Profile::from_zip_bytes(zip)?;
    let slug = request.slug.trim();
    if !crate::store::valid_slug(slug) {
        return Err(OnboardingError::InvalidSlug(format!(
            "`{slug}` is not a usable device name: use letters, digits, `-` and `_`, at most {} \
             characters",
            crate::store::MAX_SLUG_LEN
        )));
    }

    let fingerprint = profile.fingerprint();
    let existing = state.devices().slug_for_fingerprint(&fingerprint);

    // A slug held by a DIFFERENT appliance is a conflict, not an upsert.
    if let Some(occupant) = state.devices().entry(slug)
        && occupant.fingerprint != fingerprint
    {
        return Err(OnboardingError::SlugTaken(slug.to_owned()));
    }

    if let Some(previous) = existing.as_deref()
        && previous != slug
    {
        release_previous_slug(state, previous)?;
    }

    // The operator's host if non-empty, else the derived default. Trimmed only for the emptiness
    // test; the address itself is stored verbatim.
    let host = request
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_owned);
    let mut profile = profile;
    profile.set_manual_host(host.clone());

    let role = role_for_type(&profile.identity().device_type);
    tracing::info!(
        device = %slug,
        role = role.label(),
        features = profile.model().entities().len(),
        skipped = profile.skipped().len(),
        port = ?request.port,
        update = existing.is_some(),
        "parsed a profile ZIP"
    );

    state.devices().insert(
        slug,
        DeviceEntry::from_profile(slug, role, &profile).with_port(request.port),
    );

    // Persisted BEFORE the dial, so a failed connection test still leaves a device to correct.
    persist(
        state,
        slug,
        zip,
        &crate::store::StoredDevice {
            fingerprint,
            manual_host: host,
            port: request.port,
        },
    )?;

    connection_test(state, slug)
        .await
        .ok_or(OnboardingError::UnknownDevice)
}

/// Frees `previous` so the same appliance can be re-registered under a new slug.
///
/// Refuses while a task is running: a device task is detached and cannot be stopped, so a rename
/// would leave two concurrent sessions against one appliance. The operator restarts the daemon, and
/// the rename is then free.
///
/// A shadow entry means "this device has a task", reconnects included; [`launch_task`] asks the
/// same question.
fn release_previous_slug(state: &AppState, previous: &str) -> Result<(), OnboardingError> {
    if state.shadows().shadow(previous).is_some() {
        return Err(OnboardingError::DeviceBusy(previous.to_owned()));
    }
    state.devices().remove(previous);
    if let Some(store) = state.profile_store()
        && let Err(error) = store.delete(previous)
    {
        // Not fatal, but a stale pair left behind would reappear as a duplicate on the next boot.
        tracing::warn!(
            device = %previous,
            %error,
            "could not remove this device's old stored profile after a rename; it may reappear as a \
             duplicate on the next restart"
        );
    }
    tracing::info!(device = %previous, "released this slug: the same appliance is being renamed");
    Ok(())
}

/// Writes a device to the store, if one is installed.
///
/// # Errors
///
/// [`OnboardingError::Storage`] if the write fails.
fn persist(
    state: &AppState,
    slug: &str,
    zip: &[u8],
    record: &crate::store::StoredDevice,
) -> Result<(), OnboardingError> {
    let Some(store) = state.profile_store() else {
        tracing::debug!(
            device = %slug,
            "no profile store is installed, so this device will not survive a restart"
        );
        return Ok(());
    };
    store
        .save(slug, zip, record)
        .map_err(|error| OnboardingError::Storage(error.to_string()))?;
    tracing::info!(device = %slug, "stored this device's profile; it will survive a restart");
    Ok(())
}

/// Registers every persisted device, returning how many were restored.
///
/// Called once from `main`, before `spawn_device_tasks`; this function opens no socket itself. A
/// device already registered under that slug is left alone. Every restored device starts `Offline`
/// until its task connects.
pub fn load_stored_devices(state: &AppState) -> usize {
    let Some(store) = state.profile_store() else {
        return 0;
    };
    let mut restored = 0_usize;
    for device in store.load_all() {
        if state.devices().contains(&device.slug) {
            tracing::debug!(
                device = %device.slug,
                "already registered, so the stored profile was not applied over it"
            );
            continue;
        }
        let role = role_for_type(&device.profile.identity().device_type);
        tracing::info!(
            device = %device.slug,
            role = role.label(),
            features = device.profile.model().entities().len(),
            port = ?device.record.port,
            "restored this device from its stored profile"
        );
        state.devices().insert(
            &device.slug,
            DeviceEntry::from_profile(&device.slug, role, &device.profile)
                .with_port(device.record.port),
        );
        restored = restored.saturating_add(1);
    }
    restored
}

/// Forgets a device, on disk as well as in memory.
///
/// # Errors
///
/// [`OnboardingError::UnknownDevice`] if no device is registered under that slug, and
/// [`OnboardingError::DeviceBusy`] if it currently has a device task — see
/// `release_previous_slug` for why a running task cannot be discarded.
pub fn delete_device(state: &AppState, slug: &str) -> Result<(), OnboardingError> {
    if !state.devices().contains(slug) {
        return Err(OnboardingError::UnknownDevice);
    }
    if state.shadows().shadow(slug).is_some() {
        return Err(OnboardingError::DeviceBusy(slug.to_owned()));
    }
    state.devices().remove(slug);
    if let Some(store) = state.profile_store() {
        store
            .delete(slug)
            .map_err(|error| OnboardingError::Storage(error.to_string()))?;
    }
    tracing::info!(device = %slug, "removed this device from the registry and from disk");
    Ok(())
}

/// Narrows a count for the wire, saturating rather than wrapping.
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Re-runs the connection test against an operator-supplied host (the manual-IP retry).
///
/// # Errors
///
/// [`OnboardingError::UnknownDevice`] if `slug` was never onboarded.
pub async fn retry_with_manual_host(
    state: &AppState,
    slug: &str,
    host: String,
) -> Result<OnboardingResult, OnboardingError> {
    let updated = state.devices().update(slug, |entry| {
        entry.profile.set_manual_host(Some(host.clone()));
        entry.host_source = HostSource::Manual;
        (entry.fingerprint.clone(), entry.port)
    });
    let Some((fingerprint, port)) = updated else {
        return Err(OnboardingError::UnknownDevice);
    };

    // Only the record is rewritten; the ZIP on disk is unchanged. A failure is logged rather than
    // returned: the caller asked for the retry's verdict, and the in-memory correction already applies.
    if let Some(store) = state.profile_store() {
        let record = crate::store::StoredDevice {
            fingerprint,
            manual_host: Some(host),
            port,
        };
        if let Err(error) = store.save_record(slug, &record) {
            tracing::warn!(
                device = %slug,
                %error,
                "the corrected host could not be stored, so it will be lost at the next restart"
            );
        }
    }

    tracing::info!(device = %slug, "retrying the connection test with an operator-supplied host");
    connection_test(state, slug)
        .await
        .ok_or(OnboardingError::UnknownDevice)
}

/// Runs one connection test for a registered device and reports the result.
///
/// `None` only if `slug` is not registered. The dialled host is not logged: on the AES path it is
/// derived from the `haId`, which must never be logged.
async fn connection_test(state: &AppState, slug: &str) -> Option<OnboardingResult> {
    let entry = state.devices().entry(slug)?;
    let target = target_for(&entry.profile, entry.port);

    state.devices().set_state(slug, ConnectionState::Connecting);
    let verdict = tokio::time::timeout(
        CONNECTION_TEST_TIMEOUT,
        state.connection_tester().test(&target),
    )
    .await;

    let connection = match verdict {
        Ok(Ok(())) => {
            state.devices().set_state(slug, ConnectionState::Connected);
            tracing::info!(device = %slug, "connection test succeeded");
            launch_task(state, slug);
            ConnectionTest::Connected
        }
        Ok(Err(reason)) => {
            state.devices().set_state(slug, ConnectionState::Offline);
            tracing::warn!(device = %slug, %reason, "connection test failed");
            failed(reason)
        }
        Err(_elapsed) => {
            state.devices().set_state(slug, ConnectionState::Offline);
            tracing::warn!(device = %slug, "connection test timed out");
            failed(format!(
                "the connection test timed out after {} s",
                CONNECTION_TEST_TIMEOUT.as_secs()
            ))
        }
    };

    // Re-read so the summary carries the state the test just set.
    let entry = state.devices().entry(slug)?;
    Some(OnboardingResult {
        device: entry.summary(slug),
        skipped_entity_count: u32::try_from(entry.profile.skipped().len()).unwrap_or(u32::MAX),
        host_source: entry.host_source,
        connection,
    })
}

/// Starts `slug`'s long-lived device task, unless it already has one.
///
/// [`connection_test`] runs twice for one device when the operator retries with a manual IP (from
/// [`commit`] and [`retry_with_manual_host`]); without this guard the device would get two tasks
/// and two concurrent sessions. A shadow entry exists for a task's whole life, reconnects included,
/// so it answers "has a task"; `WriterRegistry` would only answer "is connected".
fn launch_task(state: &AppState, slug: &str) {
    if state.shadows().shadow(slug).is_some() {
        tracing::debug!(
            device = %slug,
            "this device already has a connection; not starting a second one"
        );
        return;
    }
    if state.device_launcher().launch(state, slug) {
        tracing::info!(device = %slug, "started this device's long-lived task at onboarding time");
    }
}

/// A failed verdict. Every failure offers the manual-IP retry, whether the host was derived or
/// typed.
fn failed(reason: impl Into<String>) -> ConnectionTest {
    ConnectionTest::Failed {
        reason: reason.into(),
        needs_manual_ip: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{role_for_type, unique_slug};
    use hc_core::device::DeviceRole;

    /// Every recognised appliance word maps to its own role, and an unknown one to `Other`.
    ///
    /// The match is case-insensitive.
    #[test]
    fn a_manifest_type_maps_to_its_role() {
        assert_eq!(role_for_type("Hob"), DeviceRole::Cooktop);
        assert_eq!(role_for_type("hob"), DeviceRole::Cooktop);
        assert_eq!(role_for_type("OVEN"), DeviceRole::Oven);
        assert_eq!(role_for_type("Dishwasher"), DeviceRole::Dishwasher);
        assert!(
            matches!(role_for_type("SousVideCircularSaw"), DeviceRole::Other(ref raw) if raw == "sousvidecircularsaw"),
            "an unrecognised word is carried as Other, ASCII-lowercased by the match above, so the UI can \
             still name it rather than showing nothing"
        );
    }

    #[test]
    fn a_taken_slug_gets_a_suffix() {
        let existing = ["cooktop".to_owned(), "cooktop-2".to_owned()];
        let slug = unique_slug(&DeviceRole::Cooktop, |candidate| {
            existing.iter().any(|taken| taken == candidate)
        });
        assert_eq!(slug, "cooktop-3", "the first free suffix is chosen");
    }
}
