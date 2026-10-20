/**
 * The browser's half of the trace stream (logs, frames, values and connection state, over SSE).
 *
 * One connection, four `event:` types — `connection`, `frame`, `value`, `log` — so the tab stays
 * under the HTTP/1.1 per-origin connection cap. `EventSource` handles reconnection and sends
 * `Last-Event-ID` on its own; `$lib/frame-log` handles the consequences (a replay, or a hole) for
 * the inspector, and `$lib/live` handles them for the appliance pages.
 *
 * The URL is built from `base`, like every other request. It carries no token: SSE is a `GET`, the
 * daemon gates only mutating methods, and a token in a query string is a token in every proxy log
 * between here and the daemon.
 */
import { apiUrl } from '$lib/api';
import type { SseEventKind, TraceFrame, TraceLevel, TraceSource } from '$lib/api-types';

/**
 * The `event:` names the daemon multiplexes onto one stream (`SseEventKind`).
 *
 * Must be exhaustive: `EventSource` dispatches by `event:` name and delivers nothing to `message`
 * when a name is set, so a kind missing here is silently dropped. [`UncoveredEventKind`] makes this
 * a check, asserted in `sse.test.ts`.
 */
export const EVENT_KINDS = [
  'connection',
  'frame',
  'value',
  'log',
] as const satisfies readonly SseEventKind[];

/**
 * The `SseEventKind` members [`EVENT_KINDS`] does **not** subscribe to — `never` when the list is
 * complete.
 *
 * A `satisfies` clause proves every listed name is a real kind but not the reverse. This alias
 * covers the reverse: add a variant to the Rust `SseEventKind`, regenerate, and this stops being
 * `never`, which fails the assertion in `sse.test.ts` with the name of the missing kind.
 */
export type UncoveredEventKind = Exclude<SseEventKind, (typeof EVENT_KINDS)[number]>;

/**
 * The severities, most severe first — the order the threshold control offers them in, and the same
 * order as the daemon's `TraceLevel::ALL`.
 */
export const TRACE_LEVELS = [
  'error',
  'warn',
  'info',
  'debug',
  'trace',
] as const satisfies readonly TraceLevel[];

/** The subsystem groups the daemon tags each event with (`TraceSource`). */
export const TRACE_SOURCES = [
  'homeconnect',
  'mqtt',
  'http',
  'daemon',
  'other',
] as const satisfies readonly TraceSource[];

/** `never` while [`TRACE_LEVELS`] is exhaustive — the [`UncoveredEventKind`] closure, for levels. */
export type UncoveredLevel = Exclude<TraceLevel, (typeof TRACE_LEVELS)[number]>;

/** `never` while [`TRACE_SOURCES`] is exhaustive. A new group must reach the filter UI, or it is
 * a group of events the operator can see but cannot isolate. */
export type UncoveredSource = Exclude<TraceSource, (typeof TRACE_SOURCES)[number]>;

/**
 * What the operator asked the daemon to send.
 *
 * Applied server-side: it is rendered into the `EventSource` URL's query string, so excluded events
 * are never serialised, never cross the ingress proxy and never parsed in the tab. The bus carries
 * everything down to `trace` regardless of `KESSEL_LOG`, which is why the wire needs a volume knob.
 */
export interface FrameFilter {
  /** The least severe level to receive: `'warn'` means warn and error. */
  readonly level: TraceLevel;
  /** The groups to receive. Empty — or all of them — means no restriction. */
  readonly sources: readonly TraceSource[];
}

/** The firehose: every level, every source. What a client that says nothing gets. */
export const ALL_FRAMES: FrameFilter = { level: 'trace', sources: [] };

/**
 * Whether `filter` narrows the stream at all.
 *
 * `seq` is assigned before the daemon filters, so a narrowed stream has holes in `seq` and the
 * dropped-events reporting in `$lib/frame-log` has to be switched off. Selecting every source counts as no
 * restriction, so toggling the last chip back on does not churn the connection.
 */
export function isFiltered(filter: FrameFilter): boolean {
  return filter.level !== 'trace' || restrictedSources(filter).length > 0;
}

/** The sources worth sending: none when the selection is empty or covers everything. */
function restrictedSources(filter: FrameFilter): readonly TraceSource[] {
  const selected = TRACE_SOURCES.filter((source) => filter.sources.includes(source));
  return selected.length === TRACE_SOURCES.length ? [] : selected;
}

/**
 * Whether `frame` is one `filter` asked for.
 *
 * The daemon applies the same predicate before it serialises anything; this is for a consumer
 * sharing a connection with someone whose requirements are broader than its own (see
 * `$lib/live-shell`).
 *
 * `TRACE_LEVELS` is ordered most-severe-first, which is what makes the threshold an index
 * comparison: `warn` admits `warn` and `error`, and nothing below them.
 */
export function matches(frame: TraceFrame, filter: FrameFilter): boolean {
  if (TRACE_LEVELS.indexOf(frame.level) > TRACE_LEVELS.indexOf(filter.level)) {
    return false;
  }
  const sources = restrictedSources(filter);
  return sources.length === 0 || sources.includes(frame.source);
}

/**
 * The one filter that satisfies every one of `filters` — the most verbose level, the widest source
 * set — or the firehose if there are none.
 *
 * This is what a shared connection is opened with. It is the union, not an intersection: a narrower
 * wire would drop events some subscriber asked for, with no way to get them back. Instead a
 * subscriber may receive more than it asked for, which [`matches`] discards.
 */
export function broadestFilter(filters: readonly FrameFilter[]): FrameFilter {
  if (filters.length === 0) {
    return ALL_FRAMES;
  }
  const level = TRACE_LEVELS.reduce((widest, candidate) =>
    filters.some((one) => one.level === candidate) ? candidate : widest,
  );
  // An empty selection means "every group", so one such subscriber widens the wire to all of them.
  const unrestricted = filters.some((one) => restrictedSources(one).length === 0);
  return {
    level,
    sources: unrestricted
      ? []
      : TRACE_SOURCES.filter((source) => filters.some((one) => one.sources.includes(source))),
  };
}

/** Whether two filters ask the daemon for the same stream, so a change need not reconnect. */
export function sameFilter(one: FrameFilter, other: FrameFilter): boolean {
  const a = restrictedSources(one);
  const b = restrictedSources(other);
  return (
    one.level === other.level && a.length === b.length && a.every((source) => b.includes(source))
  );
}

/** `''`, or the `?level=…&source=…` query the daemon parses. */
function filterQuery(filter: FrameFilter): string {
  const params = new URLSearchParams();
  if (filter.level !== 'trace') {
    params.set('level', filter.level);
  }
  const sources = restrictedSources(filter);
  if (sources.length > 0) {
    params.set('source', sources.join(','));
  }
  const query = params.toString();
  return query === '' ? '' : `?${query}`;
}

/** Just enough of `EventSource` for this module, so a test can substitute a fake. */
export interface EventSourceLike {
  addEventListener: (type: string, listener: (event: MessageEvent<string>) => void) => void;
  close: () => void;
}

export interface ConnectOptions {
  /** Called once per parsed frame, in arrival order. */
  onFrame: (frame: TraceFrame) => void;
  /** Called when a `data:` record cannot be parsed. Defaults to ignoring it. */
  onMalformed?: (raw: string) => void;
  /** What to ask the daemon for. Defaults to [`ALL_FRAMES`]. */
  filter?: FrameFilter;
  /** Defaults to the platform `EventSource`. */
  source?: (url: string) => EventSourceLike;
}

export interface FrameStream {
  close: () => void;
}

/** Opens the trace stream and delivers every frame to `onFrame`. */
export function connectFrames(options: ConnectOptions): FrameStream {
  const url = `${apiUrl('/events')}${filterQuery(options.filter ?? ALL_FRAMES)}`;
  const source = options.source?.(url) ?? new EventSource(url);

  for (const kind of EVENT_KINDS) {
    source.addEventListener(kind, (event) => {
      const frame = parseFrame(event.data);
      if (frame === null) {
        options.onMalformed?.(event.data);
        return;
      }
      options.onFrame(frame);
    });
  }

  return {
    close: () => {
      source.close();
    },
  };
}

/**
 * Parses one `data:` payload. A truncated record — the ordinary consequence of a connection dying
 * mid-event — must not throw into the event loop and kill the stream.
 */
function parseFrame(raw: string): TraceFrame | null {
  try {
    const parsed: unknown = JSON.parse(raw);
    return isFrame(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isFrame(value: unknown): value is TraceFrame {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const candidate = value as Partial<TraceFrame>;
  return (
    typeof candidate.seq === 'number' &&
    typeof candidate.kind === 'string' &&
    typeof candidate.level === 'string' &&
    typeof candidate.target === 'string' &&
    typeof candidate.message === 'string' &&
    typeof candidate.fields === 'object' &&
    candidate.fields !== null
  );
}
