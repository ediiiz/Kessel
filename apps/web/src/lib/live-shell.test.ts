import { describe, expect, it, vi } from 'vitest';
import type { TraceFrame } from '$lib/api-types';
import { type ValueUpdate } from './live';
import { debounce, LIVE_FILTER, startLive, subscribeFrames, subscribeLive } from './live-shell';

const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

/**
 * The shell's half of `$lib/live`: one `EventSource` for the app, opened by `routes/+layout.svelte`
 * and fanned out to whichever pages are mounted. `live.test.ts` covers what one frame means; this
 * file covers who hears it.
 */

/** The same stand-in `sse.test.ts` uses — jsdom implements no `EventSource` to patch. */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  /** Every connection ever opened in a case, so "one stream" can be asserted rather than assumed. */
  static opened: FakeEventSource[] = [];
  readonly listeners = new Map<string, ((event: MessageEvent<string>) => void)[]>();
  closed = false;

  constructor(readonly url: string) {
    FakeEventSource.last = this;
    FakeEventSource.opened.push(this);
  }

  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
  }

  emit(frame: TraceFrame): void {
    for (const listener of this.listeners.get(frame.kind) ?? []) {
      listener(new MessageEvent(frame.kind, { data: JSON.stringify(frame) }));
    }
  }
}

function valueFrame(seq: number, slug: string, uid: number, value: string): TraceFrame {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000,
    kind: 'value',
    level: 'debug',
    target: 'kessel::values',
    source: 'daemon',
    message: 'feature value',
    fields: { device: slug, uid: String(uid), feature_value: value },
    replayed: false,
  };
}

function connectionFrame(seq: number, state: string): TraceFrame {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000,
    kind: 'connection',
    level: 'info',
    target: 'hc_core::supervisor',
    source: 'homeconnect',
    message: 'connection state',
    fields: { device: 'dishwasher', state },
    replayed: false,
  };
}

describe('one_stream_serves_every_page_because_the_shell_owns_it', () => {
  it('opens a single connection and fans it out to whoever is subscribed', () => {
    const fleet: string[] = [];
    const devicePage: ValueUpdate[] = [];

    // The subscriber registers first: a child's `$effect` runs before its parent's, so a page is
    // listening before the layout has opened anything, and must not miss what follows.
    const leaveDevice = subscribeLive({ onValue: (update) => devicePage.push(update) });
    const stop = startLive({ source: (url: string) => new FakeEventSource(url) });
    const leaveFleet = subscribeLive({ onResync: (because) => fleet.push(because) });
    const source = FakeEventSource.last;

    source?.emit(valueFrame(1, 'dishwasher', 41, 'On'));
    source?.emit(connectionFrame(2, 'offline'));

    expect(devicePage).toStrictEqual([{ slug: 'dishwasher', uid: 41, value: 'On' }]);
    expect(fleet).toStrictEqual(['connection']);

    // Leaving a page detaches that page, not the stream: the shell still owns it.
    leaveFleet();
    source?.emit(connectionFrame(3, 'connected'));
    expect(fleet).toHaveLength(1);
    expect(source?.closed).toBe(false);

    leaveDevice();
    stop();
    expect(source?.closed).toBe(true);
  });
});

// Split from the block above only to stay inside the 50-line `max-lines-per-function` budget.
describe('one_stream_survives_navigation', () => {
  it('keeps the same connection while pages come and go', () => {
    const stop = startLive({ source: (url: string) => new FakeEventSource(url) });
    const opened = FakeEventSource.last;

    // Fleet → device → fleet. Each mount attaches and each unmount detaches.
    const heard: string[] = [];
    for (const visit of ['fleet', 'device', 'fleet']) {
      const leave = subscribeLive({ onResync: () => heard.push(visit) });
      leave();
    }
    FakeEventSource.last?.emit(connectionFrame(1, 'offline'));
    expect(heard).toStrictEqual([]);

    expect(FakeEventSource.last).toBe(opened);
    expect(opened?.closed).toBe(false);
    stop();
  });
});

describe('debounce_collapses_a_burst_of_transitions', () => {
  it('runs once for connecting/handshake/connected rather than three times', async () => {
    vi.useFakeTimers();
    let runs = 0;
    const resync = debounce(() => {
      runs += 1;
    }, 250);

    resync.call();
    resync.call();
    resync.call();
    expect(runs).toBe(0);

    await vi.advanceTimersByTimeAsync(250);
    expect(runs).toBe(1);

    resync.call();
    resync.cancel();
    await vi.advanceTimersByTimeAsync(250);
    expect(runs).toBe(1);

    vi.useRealTimers();
  });
});

/** A `log` frame at an arbitrary level — the inspector's business, and nobody else's. */
function logFrame(seq: number, level: TraceFrame['level']): TraceFrame {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000,
    kind: 'log',
    level,
    target: 'tungstenite::protocol',
    source: 'homeconnect',
    message: 'writing frame',
    fields: {},
    replayed: false,
  };
}

/**
 * The inspector shares the shell's connection. These pin the one-connection property and what it
 * costs: a wire that is the union of what everyone asked for, and a reconnect when that union
 * changes.
 */
describe('the_inspector_shares_the_shells_connection', () => {
  it('opens one stream at the broadest requirement, and retunes it as the inspector comes and goes', () => {
    FakeEventSource.opened = [];
    const source = (url: string): FakeEventSource => new FakeEventSource(url);
    const stop = startLive({ source });

    // Alone, the shell asks for what it can act on: a value event is `debug`, a transition `info`.
    expect(FakeEventSource.opened).toHaveLength(1);
    expect(FakeEventSource.last?.url).toContain(`level=${LIVE_FILTER.level}`);

    const seen: TraceFrame[] = [];
    const detach = subscribeFrames((frame) => seen.push(frame), { level: 'trace', sources: [] });

    // The inspector wants everything, so the wire widens to everything — one connection, reopened,
    // never two.
    expect(FakeEventSource.opened).toHaveLength(2);
    expect(FakeEventSource.last?.url).not.toContain('level=');
    expect(FakeEventSource.opened[0]?.closed).toBe(true);

    FakeEventSource.last?.emit(logFrame(1, 'trace'));
    expect(seen.map((frame) => frame.seq)).toStrictEqual([1]);

    // And leaving narrows it back, rather than leaving the firehose running behind a page nobody
    // is looking at.
    detach();
    expect(FakeEventSource.last?.url).toContain(`level=${LIVE_FILTER.level}`);
    stop();
  });
});

describe('a_shared_wire_still_delivers_only_what_each_subscriber_asked_for', () => {
  it('drops the events one subscriber excluded even though another needs them on the wire', () => {
    FakeEventSource.opened = [];
    const source = (url: string): FakeEventSource => new FakeEventSource(url);
    const stop = startLive({ source });

    const seen: TraceFrame[] = [];
    const detach = subscribeFrames((frame) => seen.push(frame), { level: 'warn', sources: [] });

    // `warn` is *narrower* than the shell's floor, so the wire stays at the floor and the excluded
    // events do arrive in this browser; they are dropped here rather than at the daemon, which is
    // the cost of sharing one connection.
    expect(FakeEventSource.last?.url).toContain(`level=${LIVE_FILTER.level}`);
    FakeEventSource.last?.emit(logFrame(1, 'info'));
    FakeEventSource.last?.emit(logFrame(2, 'error'));

    expect(seen.map((frame) => frame.seq)).toStrictEqual([2]);
    detach();
    stop();
  });
});
