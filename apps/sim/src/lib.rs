//! A development-only Home Connect appliance simulator.
//!
//! It serves the appliance side of the TLS-PSK WebSocket protocol for one profile ZIP, so Kessel
//! can be run end to end — onboarding, live values, writes, MQTT discovery — with no hardware.
//! It listens on loopback by default and never dials out.

#![forbid(unsafe_code)]
#![allow(
    clippy::redundant_pub_crate,
    reason = "contradicts rustc's unreachable_pub; pub(crate) is the correct visibility"
)]

pub mod protocol;
pub mod session;
pub mod state;
pub mod tls;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use hc_profile::Profile;
use parking_lot::Mutex;
use tokio::net::TcpListener;

pub use protocol::DeviceInfo;
pub use state::{AccessCase, Appliance};

/// The appliance state, shared between the connections and anyone inspecting it.
pub type SharedAppliance = Arc<Mutex<Appliance>>;

/// Everything that can stop the simulator or one of its connections.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SimError {
    /// The profile ZIP could not be read.
    #[error("profile: {0}")]
    Profile(#[from] hc_profile::ProfileError),
    /// The profile is for an AES appliance, which the simulator does not speak.
    #[error("this is an AES profile; the simulator only serves TLS-PSK profiles")]
    AesProfile,
    /// The profile `key` is not URL-safe base64.
    #[error("the profile key is not valid URL-safe base64")]
    Key,
    /// A TLS setup or handshake failure.
    #[error("TLS: {0}")]
    Tls(String),
    /// A WebSocket failure.
    #[error("WebSocket: {0}")]
    WebSocket(String),
    /// A socket failure.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// How to run the simulator.
#[derive(Debug, Clone)]
pub struct Options {
    /// The address to listen on.
    pub bind: IpAddr,
    /// The port to listen on; 0 picks a free one.
    pub port: u16,
    /// Seeds every random choice, so runs are reproducible.
    pub seed: u64,
    /// How `access` words are spelled in description changes.
    pub access_case: AccessCase,
    /// How often a live value changes.
    pub tick: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8443,
            seed: 1,
            access_case: AccessCase::Upper,
            tick: Duration::from_secs(5),
        }
    }
}

/// A bound simulator, ready to [`run`](Self::run).
#[derive(Debug)]
pub struct Simulator {
    listener: TcpListener,
    context: session::Context,
}

impl Simulator {
    /// Loads the appliance from `profile` and binds the listener.
    ///
    /// # Errors
    ///
    /// [`SimError::AesProfile`] for an AES profile, [`SimError::Key`] for a malformed key,
    /// [`SimError::Tls`] or [`SimError::Io`] if the listener cannot be set up.
    pub async fn bind(profile: &Profile, options: &Options) -> Result<Self, SimError> {
        let identity = profile.identity();
        if identity.connection_type.is_aes() {
            return Err(SimError::AesProfile);
        }
        let psk = tls::decode_psk(profile.credentials().psk64.expose_secret())?;
        let tls = Arc::new(tls::server_context(psk)?);
        let appliance = Appliance::new(profile.model().clone(), options.seed, options.access_case);
        let info = DeviceInfo {
            device_type: identity.device_type.clone(),
            brand: identity.brand.clone(),
        };
        let listener = TcpListener::bind((options.bind, options.port)).await?;
        Ok(Self {
            listener,
            context: session::Context {
                tls,
                appliance: Arc::new(Mutex::new(appliance)),
                info: Arc::new(info),
                tick: options.tick,
            },
        })
    }

    /// The address actually bound.
    ///
    /// # Errors
    ///
    /// [`SimError::Io`] if the socket cannot report it.
    pub fn local_addr(&self) -> Result<SocketAddr, SimError> {
        Ok(self.listener.local_addr()?)
    }

    /// A handle on the appliance state.
    #[must_use]
    pub fn appliance(&self) -> SharedAppliance {
        Arc::clone(&self.context.appliance)
    }

    /// Accepts connections forever, each on its own task.
    ///
    /// # Errors
    ///
    /// [`SimError::Io`] if accepting fails.
    pub async fn run(self) -> Result<(), SimError> {
        loop {
            let (tcp, peer) = self.listener.accept().await?;
            tokio::spawn(session::serve(tcp, peer, self.context.clone()));
        }
    }
}
