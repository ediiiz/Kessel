//! The trace bus: a `tracing` layer fanning events into a bounded ring buffer and a broadcast
//! channel that feeds the frame inspector. Under a flood the buffer drops the oldest event rather
//! than grow.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_core::trace::{TraceBus, TraceLevel, TraceRecord};

fn record(message: &str) -> TraceRecord {
    TraceRecord::new(TraceLevel::Info, "hc_core::test", message)
}

#[tokio::test]
async fn trace_bus_delivers_to_subscriber() {
    let bus = TraceBus::new(16);

    // Subscribe first: the bus is a live fan-out, not a replay log.
    let mut rx = bus.subscribe();
    let mut second = bus.subscribe();

    bus.emit(record("frame out"));
    bus.emit(record("frame in"));

    let first = rx
        .recv()
        .await
        .expect("subscriber receives the first event");
    assert_eq!(
        first.message, "frame out",
        "events arrive in emission order"
    );
    assert_eq!(first.level, TraceLevel::Info, "the level survives the trip");
    assert_eq!(
        first.target, "hc_core::test",
        "the target survives the trip"
    );
    assert_eq!(
        first.seq, 0,
        "sequence numbers start at zero and are assigned by the bus"
    );

    let next = rx
        .recv()
        .await
        .expect("subscriber receives the second event");
    assert_eq!(
        next.message, "frame in",
        "the second event follows the first"
    );
    assert_eq!(next.seq, 1, "sequence numbers are monotonic");

    // Every subscriber gets its own copy — the inspector may have several browser tabs open.
    let fanned = second
        .recv()
        .await
        .expect("a second subscriber receives the same event");
    assert_eq!(
        fanned.message, "frame out",
        "broadcast fan-out delivers to every subscriber"
    );

    // Emitting with nobody listening must not error or block.
    drop(rx);
    drop(second);
    bus.emit(record("nobody is listening"));

    assert_eq!(
        bus.recent().len(),
        3,
        "the ring buffer keeps events regardless of subscribers"
    );
}

#[tokio::test]
async fn trace_bus_bounded_drops_oldest() {
    const CAPACITY: usize = 4;
    const FLOOD: u64 = 1_000;

    let bus = TraceBus::new(CAPACITY);

    for i in 0..FLOOD {
        bus.emit(record(&format!("frame {i}")));
    }

    let recent = bus.recent();

    assert_eq!(
        recent.len(),
        CAPACITY,
        "the ring buffer is bounded: {FLOOD} events must not grow it past {CAPACITY}"
    );
    assert_eq!(
        recent.first().expect("buffer is non-empty").seq,
        FLOOD - CAPACITY as u64,
        "the OLDEST event is dropped, not the newest — the newest frames are the useful ones"
    );
    assert_eq!(
        recent.last().expect("buffer is non-empty").seq,
        FLOOD - 1,
        "the most recent event is retained"
    );
    assert_eq!(
        recent.last().expect("buffer is non-empty").message,
        format!("frame {}", FLOOD - 1),
        "retained events are intact, not truncated"
    );
    assert_eq!(
        bus.emitted(),
        FLOOD,
        "the bus reports the true emitted count, so drops are observable rather than silent"
    );

    // A slow subscriber must be lapped, not allowed to hold the bus's memory hostage.
    let mut slow = bus.subscribe();
    for i in 0..FLOOD {
        bus.emit(record(&format!("second flood {i}")));
    }
    let lagged = slow.recv().await;
    assert!(
        matches!(
            lagged,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
        ),
        "a subscriber that cannot keep up is told it lagged, not buffered without limit: \
         {lagged:?}"
    );
}

#[test]
fn every_event_is_stamped_with_the_wall_clock_when_emitted() {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    };
    let bus = TraceBus::new(4);

    let before = now();
    bus.emit(record("one"));
    bus.emit(record("two"));
    let after = now();

    let stamps: Vec<u128> = bus
        .recent()
        .iter()
        .map(|event| u128::from(event.timestamp_ms))
        .collect();
    assert_eq!(stamps.len(), 2);
    assert!(
        stamps.iter().all(|stamp| (before..=after).contains(stamp)),
        "{stamps:?} not within {before}..={after}"
    );
    assert!(
        stamps.windows(2).all(|pair| pair.first() <= pair.last()),
        "stamps never go backwards"
    );
}
