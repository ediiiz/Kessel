//! The device supervisor: one tokio task per device, reconnect with backoff, shadow seeding, and
//! steady-state request timeouts.
//!
//! The appliance-facing side sits behind two traits, so the supervisor can run against a fake:
//!
//! - [`DeviceLink`]: one connection attempt (in production, an `hc_transport` connect).
//! - [`DeviceSession`]: a live connection, with its decoded `/ro` initial state and its
//!   steady-state run.
//!
//! Each device task emits [`crate::device::ConnectionState`] transitions and errors to the shared
//! [`TraceBus`].

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hc_proto::ValueUpdate;
use parking_lot::Mutex;
use rand::SeedableRng as _;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

use crate::backoff::Backoff;
use crate::device::{ConnectionState, DeviceRole};
use crate::shadow::Shadow;
use crate::trace::{TraceBus, TraceLevel, TraceRecord};

/// The `tracing`/trace-bus target for supervisor events.
const TARGET: &str = "hc_core::supervisor";

/// A connection attempt failed, or a live session ended.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// The device could not be reached or the handshake failed. A normal condition for a
    /// powered-off appliance.
    #[error("device offline: {0}")]
    Offline(String),
    /// A live session dropped after being connected.
    #[error("session ended: {0}")]
    Ended(String),
}

/// A failure in the supervisor's own machinery, distinct from a [`LinkError`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SupervisorError {
    /// A steady-state request went unanswered past its deadline.
    #[error("steady-state request timed out after {0:?}")]
    RequestTimeout(Duration),
}

/// Where a live connection delivers the updates it decodes while steady state is running.
///
/// `accept` is synchronous and must not block: it runs inside the device task's steady-state loop,
/// so anything slow stalls that appliance's connection. Do not await a broker or HTTP client here;
/// hand the batch to a channel instead.
///
/// A [`ValueUpdate`] carries appliance body data. Implementors may store and decode it, but must
/// not log it or render it through `Debug`.
pub trait UpdateSink: Send {
    /// Accepts a batch of updates decoded from one steady-state read (one `NOTIFY` frame).
    fn accept(&mut self, updates: &[ValueUpdate]);
}

/// An [`UpdateSink`] that reconciles every batch into a [`Shadow`]; what [`run_device`] installs.
///
/// Callers can wrap it in their own sink to observe values on the way in.
pub struct ShadowSink<'a> {
    shadow: &'a Mutex<Shadow>,
}

impl<'a> ShadowSink<'a> {
    /// A sink writing into `shadow`.
    #[must_use]
    pub const fn new(shadow: &'a Mutex<Shadow>) -> Self {
        Self { shadow }
    }
}

impl core::fmt::Debug for ShadowSink<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Render nothing: taking the shadow lock inside `Debug` risks a deadlock.
        f.debug_struct("ShadowSink").finish_non_exhaustive()
    }
}

impl UpdateSink for ShadowSink<'_> {
    fn accept(&mut self, updates: &[ValueUpdate]) {
        // One lock acquisition per batch, not per update.
        let mut shadow = self.shadow.lock();
        for update in updates {
            shadow.reconcile(update);
        }
    }
}

/// A live connection: the seam a real `hc_transport::Session` (or a test fake) sits behind.
#[async_trait]
pub trait DeviceSession: Send {
    /// The decoded post-CONNECT `/ro` initial state, used to seed the shadow.
    fn initial_state(&self) -> Vec<ValueUpdate>;

    /// Runs steady state until the connection drops, delivering every decoded update to `sink`.
    ///
    /// Must return only when the connection has ended; [`run_device`]'s reconnect loop relies on
    /// it.
    ///
    /// # Errors
    ///
    /// [`LinkError::Ended`] when the connection drops.
    async fn run_steady_state(&mut self, sink: &mut dyn UpdateSink) -> Result<(), LinkError>;

    /// Tells the appliance this session is over — a WebSocket Close frame, not a dropped socket.
    ///
    /// Appliances track their clients in a bounded table (`NOTIFY /ci/registeredDevices`), and a
    /// client that vanishes without a Close leaves a stale entry. Infallible: implementations
    /// swallow their own transport errors. The default is a no-op for in-process fakes.
    async fn close(&mut self) {}
}

/// One connection attempt — the seam the reconnect loop drives.
#[async_trait]
pub trait DeviceLink: Send {
    /// The session type this link produces.
    type Session: DeviceSession;

    /// Attempts one connect + handshake.
    ///
    /// # Errors
    ///
    /// [`LinkError::Offline`] when the device is unreachable (the loop backs off and retries).
    async fn connect(&mut self) -> Result<Self::Session, LinkError>;
}

/// Static configuration for one device task.
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    /// The appliance role. Drives the trace-bus labels and the program-start path; gates no write.
    pub role: DeviceRole,
    /// The reconnect backoff schedule.
    pub backoff: Backoff,
    /// The jitter RNG seed — a seeded [`ChaCha8Rng`] keeps reconnect timing deterministic in tests.
    pub rng_seed: u64,
    /// A cap on connection attempts, so a test loop terminates. `None` = run forever (production).
    pub max_attempts: Option<u32>,
    /// Set to `true` to stop this device task: it closes any live session and returns.
    ///
    /// Without it a task only ends with the process, dropping the socket without the Close that
    /// [`DeviceSession::close`] sends. The signal interrupts steady state and backoff sleeps.
    /// `None` runs until the process exits.
    pub shutdown: Option<tokio::sync::watch::Receiver<bool>>,
}

/// What a finished device task reports (for assertions and diagnostics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceOutcome {
    /// How many times the device reached CONNECTED.
    pub connects: u32,
    /// How many connection attempts failed.
    pub failures: u32,
}

/// Awaits a steady-state response, enforcing the request deadline.
///
/// The transport enforces its deadline only during the handshake, so a never-answered
/// post-CONNECT request would otherwise wedge the device task.
///
/// # Errors
///
/// [`SupervisorError::RequestTimeout`] if the future does not resolve within `deadline`.
pub async fn await_response<T>(
    response: impl Future<Output = T>,
    deadline: Duration,
) -> Result<T, SupervisorError> {
    match tokio::time::timeout(deadline, response).await {
        Ok(value) => Ok(value),
        Err(_elapsed) => Err(SupervisorError::RequestTimeout(deadline)),
    }
}

/// Emits a connection-state transition to the trace bus.
fn emit_state(bus: &TraceBus, role: &DeviceRole, state: ConnectionState) {
    bus.emit(
        TraceRecord::new(TraceLevel::Info, TARGET, "connection state")
            .with_field("device", role.label())
            .with_field("state", state.as_str()),
    );
}

/// Emits a connect-failure event. The first failure of an outage is an error; later retries are
/// debug events, so a persistently offline device does not flood the error log.
fn emit_failure(bus: &TraceBus, role: &DeviceRole, first: bool, error: &LinkError) {
    let level = if first {
        TraceLevel::Error
    } else {
        TraceLevel::Debug
    };
    bus.emit(
        TraceRecord::new(level, TARGET, "connect failed")
            .with_field("device", role.label())
            .with_field("reason", error.to_string()),
    );
}

/// Runs one device's whole lifecycle.
///
/// Connect → seed shadow → steady state → reconnect with backoff, forever (or up to
/// `max_attempts`). Each device runs this in its own tokio task ([`Supervisor::spawn`]), so a
/// failure never touches a sibling.
pub async fn run_device<L>(
    mut link: L,
    mut config: DeviceConfig,
    bus: Arc<TraceBus>,
    shadow: Arc<Mutex<Shadow>>,
) -> DeviceOutcome
where
    L: DeviceLink,
{
    let mut outcome = DeviceOutcome::default();
    // Taken out of `config` so the loop can hold it mutably without borrowing the whole config.
    let mut shutdown = config.shutdown.take();
    let mut backoff = config.backoff.clone();
    let mut rng = jitter_rng(config.rng_seed);
    let mut attempt = 0_u32;
    // Whether we are inside a logged outage. A successful connect clears it, so the next outage
    // logs an error again.
    let mut in_outage = false;

    loop {
        if config.max_attempts.is_some_and(|max| attempt >= max) {
            break;
        }
        attempt = attempt.saturating_add(1);

        emit_state(&bus, &config.role, ConnectionState::Connecting);
        match link.connect().await {
            Ok(session) => {
                outcome.connects = outcome.connects.saturating_add(1);
                backoff.reset();
                in_outage = false;
                if handle_connected(session, &bus, &config.role, &shadow, shutdown.as_mut()).await {
                    break;
                }
            }
            Err(error) => {
                outcome.failures = outcome.failures.saturating_add(1);
                emit_failure(&bus, &config.role, !in_outage, &error);
                in_outage = true;
                emit_state(&bus, &config.role, ConnectionState::Offline);
            }
        }

        let delay = backoff.next_delay(&mut rng);
        if sleep_unless_shutdown(delay, shutdown.as_mut()).await {
            break;
        }
    }

    outcome
}

/// Sleeps for `delay`, unless shutdown is signalled first. `true` means stop.
///
/// The backoff sleep can be minutes long, so shutdown must not wait for it to end.
async fn sleep_unless_shutdown(
    delay: Duration,
    shutdown: Option<&mut tokio::sync::watch::Receiver<bool>>,
) -> bool {
    match shutdown {
        // No shutdown signal: just sleep.
        None => {
            tokio::time::sleep(delay).await;
            false
        }
        Some(signal) => {
            tokio::select! {
                () = tokio::time::sleep(delay) => false,
                // `wait_for` checks the current value first, so a signal sent while connecting is
                // not missed. A dropped sender also resolves it, and also means stop.
                _ = signal.wait_for(|flag| *flag) => true,
            }
        }
    }
}

/// Handles one live connection: seed the shadow from its `/ro` initial state, run steady state to
/// its end, then mark reconnecting.
async fn handle_connected<S: DeviceSession>(
    mut session: S,
    bus: &TraceBus,
    role: &DeviceRole,
    shadow: &Mutex<Shadow>,
    shutdown: Option<&mut tokio::sync::watch::Receiver<bool>>,
) -> bool {
    emit_state(bus, role, ConnectionState::Connected);
    shadow.lock().seed(&session.initial_state());
    // Every value that arrives while the connection lives reconciles into the same shadow.
    let mut sink = ShadowSink::new(shadow);
    // `run_steady_state` returns when the connection drops; a panic aborts this task alone. A
    // healthy connection never returns on its own, so shutdown has to interrupt it.
    let stopped = match shutdown {
        None => {
            let _ended = session.run_steady_state(&mut sink).await;
            false
        }
        Some(signal) => {
            tokio::select! {
                _ended = session.run_steady_state(&mut sink) => false,
                _ = signal.wait_for(|flag| *flag) => true,
            }
        }
    };
    // Close before dropping: appliances track clients in a bounded table. If the appliance hung
    // up, the close fails and is swallowed.
    session.close().await;
    // A shutdown is not a reconnect.
    if !stopped {
        emit_state(bus, role, ConnectionState::Reconnecting);
    }
    stopped
}

/// Spawns one tokio task per device; the isolation boundary.
///
/// A panicking or erroring device task cannot take down the supervisor or its siblings: each runs
/// in its own `tokio::spawn`, so a panic surfaces as a `JoinError` on that device's handle alone.
#[derive(Debug, Default)]
pub struct Supervisor {
    handles: Vec<tokio::task::JoinHandle<DeviceOutcome>>,
}

impl Supervisor {
    /// A supervisor with no devices yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns a device task and keeps its join handle.
    pub fn spawn<L>(
        &mut self,
        link: L,
        config: DeviceConfig,
        bus: Arc<TraceBus>,
        shadow: Arc<Mutex<Shadow>>,
    ) where
        L: DeviceLink + Send + 'static,
        L::Session: Send,
    {
        self.handles
            .push(tokio::spawn(run_device(link, config, bus, shadow)));
    }

    /// The device task handles, for the caller to join / await.
    #[must_use]
    pub fn handles(&self) -> &[tokio::task::JoinHandle<DeviceOutcome>] {
        &self.handles
    }

    /// Consumes the supervisor, yielding the handles to await.
    #[must_use]
    pub fn into_handles(self) -> Vec<tokio::task::JoinHandle<DeviceOutcome>> {
        self.handles
    }
}

/// Builds a seeded jitter RNG for a device (deterministic reconnect timing in tests).
#[must_use]
pub fn jitter_rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}
