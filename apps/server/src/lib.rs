//! Kessel daemon library: the axum REST + SSE API, the onboarding flow, the MQTT bridge, the
//! embedded SPA and the `ts-rs` API-type generator.
//!
//! # Shape
//!
//! | module | what it owns |
//! |---|---|
//! | [`api`] | the router and its handlers |
//! | [`auth`] | the API token and the mutating-request gate |
//! | [`config`] | the persisted daemon configuration and its environment overrides |
//! | [`hassio`] | the Home Assistant Supervisor: the autodiscovered broker and the self-restart |
//! | [`labels`] | human-readable feature labels |
//! | [`live_transport`] | the live connect + handshake and the per-device task |
//! | [`mqtt`] | the MQTT / Home Assistant bridge |
//! | [`observability`] | the `tracing` wiring: stderr and the frame-inspector bus |
//! | [`onboarding`] | the profile-ZIP flow and the connection-test seam |
//! | [`serve`] | the listener bind |
//! | [`spa`] | the embedded `SvelteKit` bundle and the ingress rules |
//! | [`sse`] | the multiplexed frame-inspector stream |
//! | [`state`] | the shared trace bus and device registry |
//! | [`store`] | on-disk persistence |
//! | [`types`] | the API view types and the `ts-rs` generator |
//! | [`write`](mod@write) | the transport seam a write reaches the wire through (the guard is `hc-core`'s) |
//!
//! # Device safety
//!
//! The HTTP and MQTT surfaces can command any paired appliance, including a cooktop or an oven;
//! nothing in this crate refuses a write by appliance type.
//!
//! [`state::AppState::new`] installs [`onboarding::NoLiveTransport`], which refuses to connect, and
//! [`onboarding::NoDeviceTasks`], which launches nothing; `main.rs` replaces both. Those two seams
//! keep an ordinary `cargo test` off real appliances.
//!
//! Every write goes through `hc_core::write::GuardedWriter`, whose transport is private, so it is
//! the only path to the wire. A device with no live session gets a writer over
//! [`write::no_live_session_transport`], which refuses; nothing is queued for a disconnected
//! appliance. What still stops a write:
//!
//! - the appliance's own `access` declaration (`hc_core::write::check_access`), default-deny on an
//!   access word it does not understand;
//! - the API token, which is off by default ([`config::effective_require_auth`]) because ingress
//!   already authenticates the browser and no port is published.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test asserts may panic; library code may not"
    )
)]

pub mod api;
pub mod auth;
pub mod config;
pub mod hassio;
pub mod labels;
pub mod live_transport;
pub mod mqtt;
pub mod observability;
pub mod onboarding;
pub mod serve;
pub mod spa;
pub mod sse;
pub mod state;
pub mod store;
pub mod types;
pub mod write;

/// Where Kessel's source and issue tracker live.
pub const SUPPORT_URL: &str = "https://github.com/ediiiz/Kessel";

/// The release version, stamped in by the image build; a local build reports the crate version.
#[must_use]
pub fn version() -> &'static str {
    option_env!("KESSEL_VERSION")
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}
