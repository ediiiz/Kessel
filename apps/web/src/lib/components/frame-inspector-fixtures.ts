/**
 * The fixtures both inspector suites drive the component with.
 *
 * Shared so the two suites cannot drift: `frame()` is the daemon's `TraceFrame` shape, and
 * `fakeStream()` records the filter every subscription was opened with. Imported only from
 * `*.test.ts`, so it never reaches the bundle.
 */
import type { TraceFrame } from '$lib/api-types';
import type { FrameFilter } from '$lib/sse';

/** A `TraceFrame` as the daemon sends it. */
export function frame(seq: number, overrides: Partial<TraceFrame> = {}): TraceFrame {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000,
    kind: 'frame',
    level: 'debug',
    target: 'hc_transport::aes_ws',
    source: 'homeconnect',
    message: 'frame',
    fields: { direction: 'tx' },
    // Live unless a case says otherwise; the inspector renders backlog and live frames the same way.
    replayed: false,
    ...overrides,
  };
}

/**
 * The component's publish scheduler, made synchronous.
 *
 * In a browser the inspector coalesces arrivals and repaints ten times a second. A test must not wait
 * for a timer, so it injects this and asserts after `flushSync`. Cancelling is a no-op because the
 * publish has already happened by the time this returns.
 */
export const immediately = (publish: () => void): (() => void) => {
  publish();
  return () => {
    // Nothing pending: `publish` ran synchronously above.
  };
};

/**
 * A stand-in for the SSE subscription. `connect()` hands the component a push function, exactly as
 * the real `EventSource` wrapper does, and the test drives it.
 *
 * It also records every filter it was subscribed with, because the filter is applied by the daemon:
 * "the operator narrowed the stream" has to mean a *new subscription*, not a hidden predicate here.
 */
export function fakeStream() {
  let push: ((received: TraceFrame) => void) | null = null;
  const filters: FrameFilter[] = [];
  const subscribe = (
    onFrame: (received: TraceFrame) => void,
    filter: FrameFilter,
  ): (() => void) => {
    push = onFrame;
    filters.push(filter);
    return () => {
      push = null;
    };
  };
  return {
    subscribe,
    filters,
    /** The filter the current subscription was opened with. */
    lastFilter: () => filters.at(-1),
    deliver: (...frames: TraceFrame[]) => {
      for (const one of frames) {
        push?.(one);
      }
    },
    /** What a dropped connection looks like from here: the listener goes away and comes back. */
    isConnected: () => push !== null,
  };
}
