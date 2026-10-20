//! `hc-core`: the per-device supervisor and the MQTT bridge.
//!
//! One tokio task per device ([`supervisor`]) reconnects with jittered exponential backoff
//! ([`backoff`]) and keeps an optimistic state [`shadow`]. Features map generically onto MQTT
//! topics and Home Assistant entities ([`mapping`], [`discovery`], [`presentation`]); the
//! [`bridge`] routes every incoming `…/set` through [`write::GuardedWriter`], the only path from
//! MQTT to an appliance transport. [`trace`] is the observability bus and [`mqtt`] the broker
//! client seam.
//!
//! Nothing in this crate refuses a write by appliance: writes can start programmes on real
//! appliances, including a cooktop and an oven. The only check is the feature's own declared
//! `access` ([`write::check_access`]). See [`mod@write`].
//!
//! A wedged-but-connected session is not detected; there is no steady-state idle watchdog.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

pub mod acks;
pub mod backoff;
pub mod bridge;
pub mod device;
pub mod discovery;
pub mod mapping;
pub mod mqtt;
pub mod presentation;
pub mod shadow;
pub mod supervisor;
pub mod trace;
pub mod write;

pub use bridge::{
    AVAILABILITY_OFFLINE, AVAILABILITY_ONLINE, DeviceBridge, SessionWriter, SetError,
    WriteFrameError, WriteSessionState, build_write_command, enum_value_for_label, enum_wire_value,
    guarded_session, guarded_session_writer, handle_set, rendered_value, take_guarded_session,
};
pub use device::{ConnectionState, DeviceRole};
pub use discovery::{
    Announcement, DerivedNames, DeviceDiscovery, DeviceInfo, DiscoveryConfig, DiscoveryMode,
    DiscoveryOrigin, FeatureNaming, device_discovery, discovery_configs,
};
pub use mapping::{
    DeviceClass, DeviceId, DeviceMapping, EntityClass, EntityMapping, Excludes, ProgramEntry,
    feature_path, normalize_program_name,
};
pub use mqtt::{LastWill, MqttClient, MqttError, RumqttcClient};
pub use shadow::{Origin, Shadow, ShadowEntry};
pub use write::{
    GuardedWriter, StartError, StartRequest, WriteCommand, WriteError, WriteTransport,
    check_access, start_command,
};

/// The crate's own name, as it appears in `[workspace.members]`. The server prints it in its
/// startup banner.
pub const CRATE_NAME: &str = "hc-core";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(
            CRATE_NAME, "hc-core",
            "crate name is part of the workspace layout"
        );
    }
}
