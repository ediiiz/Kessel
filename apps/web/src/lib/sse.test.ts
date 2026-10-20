import { describe, expect, it, vi } from 'vitest';
import type { TraceFrame } from '$lib/api-types';
import {
  ALL_FRAMES,
  connectFrames,
  EVENT_KINDS,
  isFiltered,
  TRACE_LEVELS,
  TRACE_SOURCES,
  type UncoveredEventKind,
  type UncoveredLevel,
  type UncoveredSource,
} from './sse';

const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

/**
 * A stand-in for `EventSource`, which jsdom does not implement; there is no global to patch, which
 * is why the connector takes a factory.
 */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  readonly url: string;
  readonly listeners = new Map<string, ((event: MessageEvent<string>) => void)[]>();
  closed = false;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.last = this;
  }

  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
  }

  emit(type: string, data: string): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(new MessageEvent(type, { data }));
    }
  }
}

function frame(seq: number, kind: TraceFrame['kind']): TraceFrame {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000,
    kind,
    level: 'info',
    target: 'server',
    source: 'daemon',
    message: 'hello',
    fields: {},
    replayed: false,
  };
}

describe('sse_stream_is_built_from_base_and_multiplexed', () => {
  it('opens `${base}/api/events` with no token in the URL', () => {
    const stream = connectFrames({
      source: (url: string) => new FakeEventSource(url),
      onFrame: () => {
        // The URL is the assertion here; nothing is delivered.
      },
    });

    expect(FakeEventSource.last?.url).toBe(`${INGRESS_BASE}/api/events`);
    expect(FakeEventSource.last?.url).not.toContain('token=');
    stream.close();
    expect(FakeEventSource.last?.closed).toBe(true);
  });

  it('delivers all four `event:` kinds and ignores a malformed payload', () => {
    const received: TraceFrame[] = [];
    const stream = connectFrames({
      source: (url: string) => new FakeEventSource(url),
      onFrame: (one: TraceFrame) => received.push(one),
    });
    const source = FakeEventSource.last;

    source?.emit('connection', JSON.stringify(frame(1, 'connection')));
    source?.emit('frame', JSON.stringify(frame(2, 'frame')));
    source?.emit('log', JSON.stringify(frame(3, 'log')));
    // `EventSource` dispatches by `event:` name, so a kind missing from `EVENT_KINDS` would be
    // delivered to nobody.
    source?.emit('value', JSON.stringify(frame(4, 'value')));
    // A truncated record must not take the stream down with it.
    source?.emit('frame', '{"seq":5,');

    expect(received.map((one) => one.seq)).toStrictEqual([1, 2, 3, 4]);
    expect(received.map((one) => one.kind)).toStrictEqual(['connection', 'frame', 'log', 'value']);
    stream.close();
  });
});

// Its own `describe` so the block above stays inside the 50-line `max-lines-per-function` budget.
describe('every_event_kind_has_a_subscriber', () => {
  /**
   * A type-level assertion, which is why the runtime body is trivial: `covered` only compiles while
   * `UncoveredEventKind` is `never`. Add a variant to the Rust `SseEventKind`, run the generator, and
   * `bun run --filter web check` fails here naming the kind that has no subscriber.
   */
  it('subscribes to every SseEventKind the daemon can emit', () => {
    const uncovered = null as unknown as UncoveredEventKind;
    const covered: never = uncovered;

    expect(covered).toBeNull();
    expect([...EVENT_KINDS]).toStrictEqual(['connection', 'frame', 'value', 'log']);
  });

  /** The same closure, for the two filter vocabularies the query string is built from. */
  it('offers every level and every source the daemon can send', () => {
    const level: never = null as unknown as UncoveredLevel;
    const source: never = null as unknown as UncoveredSource;

    expect(level).toBeNull();
    expect(source).toBeNull();
    // Most severe first: the order the threshold control offers, matching `TraceLevel::ALL`.
    expect([...TRACE_LEVELS]).toStrictEqual(['error', 'warn', 'info', 'debug', 'trace']);
    expect([...TRACE_SOURCES]).toStrictEqual(['homeconnect', 'mqtt', 'http', 'daemon', 'other']);
  });
});

/**
 * The filter is a *server-side* one: it travels in the query string, so the events the operator
 * excluded are never serialised, never cross the ingress proxy and never reach the tab. A browser
 * that filtered what it received would leave the firehose on the wire.
 */
describe('sse_filter_travels_in_the_query_string', () => {
  function urlFor(filter?: Parameters<typeof connectFrames>[0]['filter']): string {
    const stream = connectFrames({
      filter,
      source: (url: string) => new FakeEventSource(url),
      onFrame: () => {
        // The URL is the assertion.
      },
    });
    stream.close();
    return FakeEventSource.last?.url ?? '';
  }

  it('asks for everything by default, with no query at all', () => {
    // A bare URL, not `?level=trace&source=`: the unfiltered stream keeps its plain URL.
    expect(urlFor()).toBe(`${INGRESS_BASE}/api/events`);
    expect(urlFor(ALL_FRAMES)).toBe(`${INGRESS_BASE}/api/events`);
    expect(isFiltered(ALL_FRAMES)).toBe(false);
  });

  it('sends the level threshold and the selected sources', () => {
    expect(urlFor({ level: 'warn', sources: [] })).toBe(`${INGRESS_BASE}/api/events?level=warn`);
    expect(urlFor({ level: 'trace', sources: ['mqtt'] })).toBe(
      `${INGRESS_BASE}/api/events?source=mqtt`,
    );
    // Canonical order, not selection order: the URL is the connection's identity, and two
    // equivalent selections that spell it differently would reconnect the stream for nothing.
    expect(urlFor({ level: 'debug', sources: ['mqtt', 'homeconnect'] })).toBe(
      `${INGRESS_BASE}/api/events?level=debug&source=homeconnect%2Cmqtt`,
    );
    expect(isFiltered({ level: 'trace', sources: ['mqtt'] })).toBe(true);
    expect(isFiltered({ level: 'warn', sources: [] })).toBe(true);
  });

  it('treats every source selected as no source restriction', () => {
    // Otherwise the URL churns — and so does the `EventSource` — every time the operator toggles
    // the last chip back on.
    expect(urlFor({ level: 'trace', sources: [...TRACE_SOURCES] })).toBe(
      `${INGRESS_BASE}/api/events`,
    );
    expect(isFiltered({ level: 'trace', sources: [...TRACE_SOURCES] })).toBe(false);
  });
});
