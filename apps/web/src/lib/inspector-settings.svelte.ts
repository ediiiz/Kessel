/**
 * What the frame inspector remembers between visits.
 *
 * The inspector's level and source controls are applied by the daemon (rendered into the
 * `EventSource` URL by `$lib/sse`'s [`FrameFilter`]), so changing them is a reconnect. Under Home
 * Assistant ingress every in-app navigation is a full page load, so without persistence an operator
 * who set a filter and navigated away would come back to the unfiltered stream.
 *
 * {@link inspectorSettings} is the singleton the route uses, restored from `localStorage` at
 * import. `FrameInspector` takes an instance as a prop so tests do not share one case's filter with
 * the next.
 *
 * Persistence is best-effort: `localStorage` throws when site data is blocked, which is realistic
 * inside the ingress iframe, so every read and write is wrapped and a failure falls back to the
 * defaults. Stored values are validated rather than trusted, since an unrecognised level would be
 * sent to the daemon as a query it cannot parse; unknown levels and sources are dropped.
 */
import type { TraceLevel, TraceSource } from '$lib/api-types';
import { type FrameFilter, TRACE_LEVELS, TRACE_SOURCES } from '$lib/sse';

/** Where the preference lives. One key, one JSON object, so a new field costs no migration. */
export const INSPECTOR_STORAGE_KEY = 'kessel.inspector';

/** The persisted shape. Plain data: no proxies, nothing reactive, exactly what is serialised. */
export interface StoredInspectorSettings {
  /** The least severe level to receive; `'trace'` is everything. */
  readonly level: TraceLevel;
  /** The subsystem groups to receive. Empty means all of them. */
  readonly sources: readonly TraceSource[];
  /** Whether the log scrolls itself to the newest row. */
  readonly follow: boolean;
}

/** The firehose, tailing — what an operator who has never touched the controls gets. */
export const INSPECTOR_DEFAULTS: StoredInspectorSettings = {
  level: 'trace',
  sources: [],
  follow: true,
};

function isLevel(value: unknown): value is TraceLevel {
  return typeof value === 'string' && (TRACE_LEVELS as readonly string[]).includes(value);
}

function isSource(value: unknown): value is TraceSource {
  return typeof value === 'string' && (TRACE_SOURCES as readonly string[]).includes(value);
}

/**
 * Whatever was in storage, narrowed to something the daemon will accept.
 *
 * Field by field rather than all-or-nothing: a record with a good level and one unrecognised source
 * keeps the level.
 */
export function parseInspectorSettings(raw: unknown): StoredInspectorSettings {
  if (raw === null || typeof raw !== 'object') {
    return INSPECTOR_DEFAULTS;
  }
  const record = raw as Record<string, unknown>;
  const sources = Array.isArray(record.sources) ? record.sources.filter(isSource) : [];
  return {
    level: isLevel(record.level) ? record.level : INSPECTOR_DEFAULTS.level,
    // De-duplicated and in the daemon's own order, so two selections that mean the same stream
    // produce the same query string and cannot churn the connection.
    sources: TRACE_SOURCES.filter((source) => sources.includes(source)),
    follow: typeof record.follow === 'boolean' ? record.follow : INSPECTOR_DEFAULTS.follow,
  };
}

/** What was stored, or the defaults. Never throws: see the module header. */
export function loadInspectorSettings(): StoredInspectorSettings {
  try {
    const raw = globalThis.localStorage?.getItem(INSPECTOR_STORAGE_KEY);
    return raw === null || raw === undefined
      ? INSPECTOR_DEFAULTS
      : parseInspectorSettings(JSON.parse(raw));
  } catch {
    // Blocked storage, or text that is not JSON. Either way the inspector still works.
    return INSPECTOR_DEFAULTS;
  }
}

/** Records a preference. Never throws, for the same reason {@link loadInspectorSettings} does not. */
export function saveInspectorSettings(settings: StoredInspectorSettings): void {
  try {
    globalThis.localStorage?.setItem(INSPECTOR_STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // A preference that cannot be remembered still applies for this page.
  }
}

export class InspectorSettings {
  /** The verbosity the stream is opened with. */
  level = $state<TraceLevel>(INSPECTOR_DEFAULTS.level);
  /**
   * The selected source groups.
   *
   * `string[]`, not `TraceSource[]`, because it is `bind:`-ed straight to a `ToggleGroup`; writing
   * back through `onValueChange` instead would make Svelte warn that two things own the array. The
   * narrowing happens in {@link filter}.
   */
  sources = $state<string[]>([]);
  /** Whether the log is pinned to its newest row. Flipped by scrolling as well as by the button. */
  follow = $state(INSPECTOR_DEFAULTS.follow);

  constructor(initial: StoredInspectorSettings = INSPECTOR_DEFAULTS) {
    this.level = initial.level;
    this.sources = [...initial.sources];
    this.follow = initial.follow;
  }

  /** What leaves for the daemon: a plain [`FrameFilter`], with anything unrecognised dropped. */
  get filter(): FrameFilter {
    return { level: this.level, sources: this.sources.filter((one) => isSource(one)) };
  }

  /** A plain snapshot, out of the reactive proxy — what {@link saveInspectorSettings} takes. */
  snapshot(): StoredInspectorSettings {
    return { level: this.level, sources: this.filter.sources, follow: this.follow };
  }
}

/** The instance the route renders with, restored from storage. */
export const inspectorSettings = new InspectorSettings(loadInspectorSettings());
