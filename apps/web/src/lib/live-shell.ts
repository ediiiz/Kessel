/**
 * The app's one connection to the trace stream, and everybody who reads it.
 *
 * `$lib/sse` is the transport, `$lib/live` turns a frame into "this value changed" or "refetch", and
 * this module decides who is connected and what the wire is asked to carry. It is the only stateful
 * part: module-level state that outlives every page.
 *
 * # One connection, opened at the broadest requirement
 *
 * The shell wants the events it can act on, which are `debug` and worse ({@link LIVE_FILTER}); the
 * inspector wants whatever the operator asked for, up to `trace`. The connection is opened with the
 * union of those, never an intersection (which would drop events a subscriber asked for), and
 * reopened when the union changes, in practice when the inspector page arrives or leaves. Each
 * subscriber then receives only what its own filter admits (`$lib/sse`'s `matches`).
 *
 * # Held by the shell, not by a page
 *
 * One `EventSource` per page would mean a new connection and backlog replay on every navigation,
 * counting against the browser's six-connections-per-origin limit, with a window between close and
 * reopen in which a transition is lost. Held by the shell, it is opened once and outlives every
 * navigation.
 */
import type { TraceFrame } from '$lib/api-types';
import { connectLive, type Resync, type ValueUpdate } from '$lib/live';
import {
  broadestFilter,
  type EventSourceLike,
  type FrameFilter,
  type FrameStream,
  matches,
  sameFilter,
} from '$lib/sse';

/** What one page wants from the shared stream. Both halves are optional: the fleet screen renders
 * no values, and a page that only renders values needs no resync. */
export interface LiveHandlers {
  onValue?: (update: ValueUpdate) => void;
  onResync?: (because: Resync) => void;
}

/**
 * Everyone currently attached. A `Set` of the handler objects themselves, so leaving is `delete`
 * rather than an index that shifts when someone else leaves first.
 */
const attached = new Set<LiveHandlers>();

/**
 * What the shell itself needs on the wire, and why it is not the firehose.
 *
 * Everything this module interprets is emitted at `debug` or worse: value events are `Debug` and
 * connection transitions are `Info`. Asking for `trace` would pull the whole handshake firehose into
 * every page only for [`interpret`] to discard nearly all of it.
 *
 * The cost of narrowing (see [`LiveOptions.filter`]): a filtered wire has holes in `seq`, so the
 * shell cannot tell "I was lapped" from "I filtered that out", and gap-driven resync is off whenever
 * the wire is narrower than everything. Connection transitions still arrive and still resync, and a
 * narrower wire is itself the main defence against being lapped.
 */
export const LIVE_FILTER: FrameFilter = { level: 'debug', sources: [] };

/** One consumer of raw frames — the inspector — and what it asked the daemon for. */
interface RawSubscriber {
  readonly onFrame: (frame: TraceFrame) => void;
  readonly filter: FrameFilter;
}

/** The raw-frame consumers, whose requirements widen the wire. */
const rawSubscribers = new Set<RawSubscriber>();

/** The app's one stream, and how many shells are holding it open (one, in practice). */
let shared: FrameStream | null = null;
let holders = 0;
/** What [`shared`] was opened with, so a retune knows whether anything actually changed. */
let wire: FrameFilter = LIVE_FILTER;
/** The `EventSource` factory the shell handed in, kept for the reopens it does not initiate. */
let sourceFactory: ((url: string) => EventSourceLike) | undefined;

/** What the wire has to carry to satisfy the shell and every raw subscriber at once. */
function required(): FrameFilter {
  return broadestFilter([LIVE_FILTER, ...[...rawSubscribers].map((one) => one.filter)]);
}

/** Opens the one connection at `filter`, wiring both halves of it. */
function open(filter: FrameFilter): void {
  wire = filter;
  shared = connectLive({
    source: sourceFactory,
    filter,
    onFrame: (frame) => {
      for (const subscriber of rawSubscribers) {
        // The wire is the *broadest* requirement, so a subscriber can be handed events it did not
        // ask for. Its own filter is applied here rather than left to the consumer.
        if (matches(frame, subscriber.filter)) {
          subscriber.onFrame(frame);
        }
      }
    },
    onValue: (update) => {
      for (const handlers of attached) {
        handlers.onValue?.(update);
      }
    },
    onResync: (because) => {
      for (const handlers of attached) {
        handlers.onResync?.(because);
      }
    },
  });
}

/**
 * Reopens the connection if what is needed on the wire has changed.
 *
 * A reconnect costs the daemon's retained ring being replayed and a window in which a transition can
 * be missed, so the pages are asked to resync — but only when a *live* stream was replaced, never
 * for the first open, where the page has just loaded the data anyway.
 *
 * Does nothing while no shell holds the stream: a raw subscriber may attach first (a child's
 * `$effect` runs before its parent's), and [`startLive`] reads [`required`] when it opens.
 */
function retune(): void {
  const next = required();
  if (shared === null || sameFilter(next, wire)) {
    return;
  }
  shared.close();
  open(next);
  for (const handlers of attached) {
    handlers.onResync?.('reconnect');
  }
}

/**
 * Attaches a raw-frame consumer (the frame inspector) to the app's one stream, and widens the wire
 * to cover what it asked for. Returns its teardown, which narrows it back.
 *
 * The inspector shares the shell's connection rather than opening its own, which would mean two
 * connections, two backlog replays and two copies of the stream parsed in the same tab.
 *
 * The cost: the wire is the union of every subscriber's filter, so narrowing the inspector below
 * [`LIVE_FILTER`] narrows what this browser renders rather than what the daemon sends; those events
 * are dropped by [`matches`] on the way in. Above that floor the daemon filters as before.
 */
export function subscribeFrames(
  onFrame: (frame: TraceFrame) => void,
  filter: FrameFilter,
): () => void {
  const subscriber: RawSubscriber = { onFrame, filter };
  rawSubscribers.add(subscriber);
  retune();
  return () => {
    rawSubscribers.delete(subscriber);
    retune();
  };
}

/**
 * Opens the app's one live stream. **Called by the shell** (`routes/+layout.svelte`); returns its
 * teardown. See the module header for why it is the shell's and not a page's.
 *
 * `holders` rather than a bare flag so a second shell — a test, or a future embedded view — cannot
 * close the stream the first one is still using.
 */
export function startLive(options: { source?: (url: string) => EventSourceLike } = {}): () => void {
  holders += 1;
  sourceFactory = options.source;
  if (shared === null) {
    // `required()` rather than `LIVE_FILTER`: the inspector's page may have attached first, since a
    // child's `$effect` runs before its parent's, and opening narrow only to reopen would cost the
    // daemon a connection churn on every load of `#/inspector`.
    open(required());
  }

  let released = false;
  return () => {
    if (released) {
      return;
    }
    released = true;
    holders -= 1;
    if (holders === 0) {
      shared?.close();
      shared = null;
    }
  };
}

/**
 * Attaches one page to the shared stream for as long as it is mounted. Returns its teardown.
 *
 * Deliberately independent of [`startLive`]'s lifetime in both directions: a page may attach before
 * the shell has opened anything — a child's `$effect` runs before its parent's — and detaching the
 * last page does **not** close the stream, because the next page is a navigation away and the
 * daemon should not see a connection churn for it.
 */
export function subscribeLive(handlers: LiveHandlers): () => void {
  attached.add(handlers);
  return () => {
    attached.delete(handlers);
  };
}

/**
 * Collapses a burst of resync requests into one call.
 *
 * A device coming up emits `connecting` → `handshake` → `connected` in quick succession, and every
 * one of them is a reason to refetch. Three refetches for one transition is the difference between
 * a live page and a page that hammers the daemon.
 */
export function debounce(action: () => void, ms: number): { call: () => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return {
    call: () => {
      if (timer !== null) {
        clearTimeout(timer);
      }
      timer = setTimeout(() => {
        timer = null;
        action();
      }, ms);
    },
    cancel: () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    },
  };
}

/** How long to wait for a burst of connection transitions to settle. */
export const RESYNC_DEBOUNCE_MS = 250;
