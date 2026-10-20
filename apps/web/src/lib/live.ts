/**
 * Live state for the appliance pages: the half of the SSE stream that is not the inspector.
 *
 * `$lib/sse` is the transport and `$lib/frame-log` is the inspector's accumulator. This module
 * reads the same connection and turns it into "this entity's value is now X" and "refetch,
 * something moved".
 *
 * # Two kinds of update
 *
 * A value event is applied directly. It carries the device slug, the uid as a decimal string, and
 * the value rendered the same way REST and MQTT render it (an enumeration as its declared member
 * name, so it already matches `EntityDescriptor.options`). Re-deriving it in the browser would be a
 * second renderer to drift.
 *
 * A connection event is only a resync trigger. Its `state` is a `DeviceConnectionState`, but its
 * `device` field is the role class (`"dishwasher"`), not the slug the SPA keys devices by, so with
 * two appliances of one role the events are indistinguishable. A connection event therefore means
 * "ask the daemon", and `GET /api/devices` answers from memory.
 *
 * # The backlog is not news
 *
 * The stream opens by replaying the daemon's retained ring buffer in front of the live feed. That
 * suits the inspector's scrollback but not a page that reacts, which would refetch for history it
 * had just loaded. `TraceFrame.replayed` marks the backlog; a replayed connection event is dropped.
 *
 * # A gap is also a resync
 *
 * `TraceFrame.seq` is gap-free, so a gap means the browser was lapped and some value event is gone.
 * Rather than leave an entity stuck at a stale value, a gap becomes a refetch.
 */
import type { TraceFrame } from '$lib/api-types';
import {
  ALL_FRAMES,
  connectFrames,
  type EventSourceLike,
  type FrameFilter,
  type FrameStream,
  isFiltered,
} from '$lib/sse';

/** The field names the daemon marks a value event with. */
const VALUE_FIELD = 'feature_value';
const VALUE_DEVICE_FIELD = 'device';
const VALUE_UID_FIELD = 'uid';

/** One feature's new value, addressed the way the SPA addresses features. */
export interface ValueUpdate {
  readonly slug: string;
  readonly uid: number;
  /** Already rendered by the daemon. Never re-parsed here. */
  readonly value: string;
}

/**
 * What one frame means to a page. `null` for everything else on the stream — `log` and `frame`
 * events are the inspector's business and must not cause a refetch, or a chatty handshake would put
 * the appliance pages in a refetch loop.
 */
export type LiveEvent =
  | { readonly live: 'value'; readonly update: ValueUpdate }
  | { readonly live: 'resync'; readonly because: Resync };

/**
 * Why a page is being asked to refetch.
 *
 * `'connection'` is an appliance transition, `'gap'` is this browser having been lapped, and
 * `'reconnect'` is the shared stream having been reopened under it — see [`subscribeFrames`], which
 * retunes the wire when the inspector arrives or leaves. No consumer branches on this; it exists so
 * the reason in a trace is the true one.
 */
export type Resync = 'connection' | 'gap' | 'reconnect';

/**
 * Interprets one frame. Pure, so the interesting cases are testable without a stream.
 *
 * A malformed value event — a `uid` that is not a number, a missing slug — returns `null` rather
 * than a partial update. The daemon always sends all three fields together; a frame that disagrees
 * is not something to guess at.
 */
export function interpret(frame: TraceFrame): LiveEvent | null {
  if (frame.kind === 'connection') {
    // A replayed transition is history, not news: refetching for it would re-load data the page's
    // own `load()` has just fetched.
    return frame.replayed ? null : { live: 'resync', because: 'connection' };
  }
  if (frame.kind !== 'value') {
    return null;
  }

  const slug = frame.fields[VALUE_DEVICE_FIELD];
  const rawUid = frame.fields[VALUE_UID_FIELD];
  const value = frame.fields[VALUE_FIELD];
  if (slug === undefined || rawUid === undefined || value === undefined) {
    return null;
  }

  // `Number.parseInt` would accept `"41abc"`. The daemon writes `u16::to_string`, so anything else
  // is a frame we do not understand.
  const uid = Number(rawUid);
  if (!Number.isInteger(uid)) {
    return null;
  }

  return { live: 'value', update: { slug, uid, value } };
}

export interface LiveOptions {
  /** A feature value changed. */
  onValue: (update: ValueUpdate) => void;
  /** Something moved that this stream cannot describe precisely — refetch. */
  onResync: (because: Resync) => void;
  /** Defaults to the platform `EventSource`, via `$lib/sse`. */
  source?: (url: string) => EventSourceLike;
  /**
   * Every frame the wire carried, before interpretation — the inspector's half of the same
   * connection. See [`subscribeFrames`].
   */
  onFrame?: (frame: TraceFrame) => void;
  /**
   * What to ask the daemon for. Defaults to the firehose.
   *
   * Gap reporting switches itself off when this narrows the stream: `seq` is assigned before the
   * daemon filters and is not renumbered, so on a filtered wire the holes are the events this client
   * excluded. Reading them as "I was lapped" would put a page into a permanent refetch loop.
   */
  filter?: FrameFilter;
}

/**
 * Subscribes to the stream and reports live changes.
 *
 * Sequence tracking is here rather than in `$lib/frame-log` because the two consumers want opposite
 * things from a gap: the inspector renders it, a page recovers from it. `EventSource` reconnects and
 * may replay, so a `seq` at or below the highest seen is a duplicate and is ignored — replaying a
 * value update would be harmless, but replaying it as a *gap* would not.
 */
export function connectLive(options: LiveOptions): FrameStream {
  let highest: number | null = null;
  const detectGaps = !isFiltered(options.filter ?? ALL_FRAMES);

  return connectFrames({
    source: options.source,
    filter: options.filter,
    onFrame: (frame: TraceFrame) => {
      // Raw first, and unconditionally: the inspector renders the backlog and the duplicates too,
      // and it does its own sequence work (`$lib/frame-log`).
      options.onFrame?.(frame);
      if (highest !== null && frame.seq <= highest) {
        return;
      }
      const missed = detectGaps && highest !== null && frame.seq > highest + 1;
      highest = frame.seq;

      const event = interpret(frame);
      if (missed) {
        options.onResync('gap');
        // Still apply this frame: it is the newest value for its uid, and the refetch it just
        // triggered is in flight rather than complete.
      }
      if (event === null) {
        return;
      }
      if (event.live === 'value') {
        options.onValue(event.update);
      } else {
        options.onResync(event.because);
      }
    },
  });
}
