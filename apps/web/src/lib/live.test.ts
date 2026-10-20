import { describe, expect, it, vi } from 'vitest';
import type { TraceFrame } from '$lib/api-types';
import { connectLive, interpret, type ValueUpdate } from './live';

const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

/** The same stand-in `sse.test.ts` uses — jsdom implements no `EventSource` to patch. */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  readonly listeners = new Map<string, ((event: MessageEvent<string>) => void)[]>();
  closed = false;

  constructor(readonly url: string) {
    FakeEventSource.last = this;
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

/** A value event shaped as the daemon emits one. */
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
    // `device` is the ROLE CLASS, not the slug — the reason a connection event is only a resync.
    fields: { device: 'dishwasher', state },
    replayed: false,
  };
}

/** The same event as the daemon replays it to a subscriber that arrived afterwards. */
function retained(frame: TraceFrame): TraceFrame {
  return { ...frame, replayed: true };
}

describe('interpret_maps_a_frame_to_what_a_page_should_do', () => {
  it('reads a value event as an addressed update, with the daemon-rendered value verbatim', () => {
    // "Relative" is an enum member NAME, already rendered to match `EntityDescriptor.options`.
    expect(interpret(valueFrame(7, 'dishwasher', 41, 'Relative'))).toStrictEqual({
      live: 'value',
      update: { slug: 'dishwasher', uid: 41, value: 'Relative' },
    });
  });

  it('reads a connection event as a resync, never as a state to apply', () => {
    // The state is `connected` and is deliberately NOT carried through: the event cannot say which
    // device it belongs to, so applying it would be a guess.
    expect(interpret(connectionFrame(8, 'connected'))).toStrictEqual({
      live: 'resync',
      because: 'connection',
    });
  });

  it('ignores the inspector kinds, so a chatty handshake causes no refetch', () => {
    const log: TraceFrame = {
      seq: 9,
      timestamp_ms: 1_700_000_000_000,
      kind: 'log',
      level: 'info',
      target: 'kessel',
      source: 'daemon',
      message: 'listening',
      fields: {},
      replayed: false,
    };
    const wire: TraceFrame = { ...log, seq: 10, kind: 'frame', fields: { direction: 'tx' } };

    expect(interpret(log)).toBeNull();
    expect(interpret(wire)).toBeNull();
  });

  it('drops a value event whose fields are not the three the daemon sends together', () => {
    const noSlug = valueFrame(11, 'x', 41, 'On');
    delete noSlug.fields.device;
    const badUid: TraceFrame = {
      ...valueFrame(12, 'dishwasher', 41, 'On'),
      fields: { device: 'dishwasher', uid: '41abc', feature_value: 'On' },
    };

    expect(interpret(noSlug)).toBeNull();
    // `Number.parseInt` would accept this and produce uid 41 for a frame we cannot read.
    expect(interpret(badUid)).toBeNull();
  });
});

// Split from the block above only to stay inside the 50-line `max-lines-per-function` budget.
describe('interpret_tells_history_from_news', () => {
  it('reads a REPLAYED connection event as nothing at all', () => {
    // The daemon replays its retained ring buffer to every new subscriber before the live feed, so
    // this transition happened before the page's own load. Acting on it would refetch data that is
    // already newer than the event.
    expect(interpret(retained(connectionFrame(8, 'connected')))).toBeNull();
  });
});

describe('connectLive_tracks_the_sequence_because_a_hole_means_a_stale_value', () => {
  it('delivers values in order and asks for no resync while the sequence is intact', () => {
    const values: ValueUpdate[] = [];
    const resyncs: string[] = [];
    const stream = connectLive({
      source: (url: string) => new FakeEventSource(url),
      onValue: (update) => values.push(update),
      onResync: (because) => resyncs.push(because),
    });
    const source = FakeEventSource.last;

    source?.emit(valueFrame(1, 'dishwasher', 41, 'Relative'));
    source?.emit(valueFrame(2, 'dishwasher', 42, 'On'));

    expect(values).toStrictEqual([
      { slug: 'dishwasher', uid: 41, value: 'Relative' },
      { slug: 'dishwasher', uid: 42, value: 'On' },
    ]);
    expect(resyncs).toStrictEqual([]);
    stream.close();
    expect(FakeEventSource.last?.closed).toBe(true);
  });
});

// Split from the block above only to stay inside the 50-line `max-lines-per-function` budget.
describe('connectLive_recovers_from_a_hole_and_from_a_replay', () => {
  it('asks for a resync when `seq` skips, and still applies the frame that revealed the hole', () => {
    const values: ValueUpdate[] = [];
    const resyncs: string[] = [];
    const stream = connectLive({
      source: (url: string) => new FakeEventSource(url),
      onValue: (update) => values.push(update),
      onResync: (because) => resyncs.push(because),
    });
    const source = FakeEventSource.last;

    source?.emit(valueFrame(1, 'dishwasher', 41, 'Relative'));
    // seq 2 and 3 were dropped — the browser was lapped by the broadcast channel.
    source?.emit(valueFrame(4, 'dishwasher', 42, 'Off'));

    expect(resyncs).toStrictEqual(['gap']);
    expect(values.at(-1)).toStrictEqual({ slug: 'dishwasher', uid: 42, value: 'Off' });
    stream.close();
  });

  it('ignores a replayed frame, so a reconnect does not fabricate a gap', () => {
    const values: ValueUpdate[] = [];
    const resyncs: string[] = [];
    const stream = connectLive({
      source: (url: string) => new FakeEventSource(url),
      onValue: (update) => values.push(update),
      onResync: (because) => resyncs.push(because),
    });
    const source = FakeEventSource.last;

    source?.emit(valueFrame(5, 'dishwasher', 41, 'On'));
    // `EventSource` reconnected and the daemon replayed its retained backlog.
    source?.emit(valueFrame(5, 'dishwasher', 41, 'On'));
    source?.emit(valueFrame(4, 'dishwasher', 40, 'stale'));

    expect(values).toHaveLength(1);
    expect(resyncs).toStrictEqual([]);
    stream.close();
  });
});

describe('connectLive_does_not_react_to_the_backlog_it_is_handed_at_connect', () => {
  it('asks for no resync for the retained transitions, and still reacts to what follows', () => {
    const resyncs: string[] = [];
    const stream = connectLive({
      source: (url: string) => new FakeEventSource(url),
      onValue: () => {
        // Not this test's subject.
      },
      onResync: (because) => resyncs.push(because),
    });
    const source = FakeEventSource.last;

    // What a browser is handed the moment it subscribes: the daemon's whole history for this device.
    source?.emit(retained(connectionFrame(1, 'connecting')));
    source?.emit(retained(connectionFrame(2, 'connected')));
    source?.emit(retained(valueFrame(3, 'dishwasher', 41, 'Relative')));

    expect(resyncs).toStrictEqual([]);

    // And then the appliance actually drops, live. That one is worth a refetch.
    source?.emit(connectionFrame(4, 'offline'));
    expect(resyncs).toStrictEqual(['connection']);
    stream.close();
  });
});

// Split from the block above only to stay inside the 50-line `max-lines-per-function` budget.
describe('connectLive_still_reports_a_hole_in_the_backlog', () => {
  it('reports a gap wherever it happened, because a hole is not a claim about when', () => {
    const values: ValueUpdate[] = [];
    const resyncs: string[] = [];
    const stream = connectLive({
      source: (url: string) => new FakeEventSource(url),
      onValue: (update) => values.push(update),
      onResync: (because) => resyncs.push(because),
    });
    const source = FakeEventSource.last;

    // A gap is not a claim about *when* something happened — it says a value update is missing from
    // what this browser holds, and the page's copy of that entity is stale either way.
    source?.emit(retained(valueFrame(1, 'dishwasher', 41, 'Relative')));
    source?.emit(retained(valueFrame(9, 'dishwasher', 42, 'On')));

    expect(resyncs).toStrictEqual(['gap']);
    expect(values).toHaveLength(2);
    stream.close();
  });
});
