//! The daemon shuts down gracefully on `SIGTERM`, not only on Ctrl-C.
//!
//! The Supervisor stops an add-on with `SIGTERM`, whose default disposition kills the process
//! (exit 143), so every add-on restart would otherwise take the hard path.
//!
//! Signal disposition is process-global, so this is an end-to-end test: it spawns the real binary
//! (`CARGO_BIN_EXE_kessel`), signals it and reads the exit status, which is what a Supervisor sees.
//!
//! Touches no device: `KESSEL_PROFILE_DIR` is an empty temporary directory, so no device task
//! starts; `KESSEL_MQTT_AUTODISCOVER=false` and no `KESSEL_MQTT_HOST` mean no broker connection. The
//! only socket is the daemon's own listener on an ephemeral port.

#![cfg(unix)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the daemon to say it is listening before giving up on the test.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a graceful shutdown may take. Generous: what is being asserted is that it happens at all.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// A free TCP port, released immediately.
///
/// Racy in principle, but a fixed port would collide with a developer's running daemon; a lost race
/// shows up as a bind error in the captured log, not a silent pass.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
    listener.local_addr().expect("the bound address").port()
}

#[test]
fn sigterm_shuts_the_daemon_down_gracefully() {
    let workspace = tempfile::tempdir().expect("a temporary directory");
    let port = free_port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_kessel"))
        .env("KESSEL_PORT", port.to_string())
        // Empty: nothing is restored, so no appliance is contacted (see the module docs).
        .env("KESSEL_PROFILE_DIR", workspace.path().join("profiles"))
        .env("KESSEL_CONFIG_FILE", workspace.path().join("config.json"))
        .env("KESSEL_TOKEN_FILE", workspace.path().join("kessel.token"))
        // No Supervisor to ask, and asking would cost this test the 10 s timeout in `hassio::send`.
        .env("KESSEL_MQTT_AUTODISCOVER", "false")
        .env("KESSEL_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the daemon binary is built by cargo for this test");

    let stderr = child.stderr.take().expect("a piped stderr");
    let mut reader = BufReader::new(stderr);
    let mut log = String::new();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut listening = false;
    while Instant::now() < deadline {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                log.push_str(&line);
                if line.contains("kessel listening") {
                    listening = true;
                    break;
                }
            }
            Err(error) => panic!("reading the daemon's log: {error}\n{log}"),
        }
    }
    assert!(
        listening,
        "the daemon never reported that it was listening, so this test would be asserting about a \
         process that had already failed for another reason. Its log:\n{log}"
    );

    // The Supervisor's stop: `SIGTERM` to the process, sent with `kill(1)` to avoid a `libc`
    // dependency for one call.
    let signalled = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill(1) is available");
    assert!(signalled.success(), "SIGTERM could not be delivered");

    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    let status = loop {
        match child.try_wait().expect("waiting on the daemon") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!(
                    "the daemon did not exit within {SHUTDOWN_TIMEOUT:?} of SIGTERM. Its log:\n{log}"
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    // 143 (128 + SIGTERM) means nothing handled the signal. A graceful shutdown means
    // `axum::serve` returned, `main` returned `Ok(())`, and the process chose its own exit status.
    assert_eq!(
        status.code(),
        Some(0),
        "SIGTERM must reach a handler and run `axum::serve`'s graceful shutdown, so the daemon exits \
         0. A status of None means it was killed by a signal (143 = 128 + SIGTERM is the unhandled \
         case). This is how the Supervisor stops an add-on, so it is the path every restart takes — \
         including the one the Settings page offers. Its log:\n{log}"
    );
}
