/**
 * Everything `FrameInspector.svelte` renders from that is not markup or state: labels, the row key,
 * the severity tone, and the redraw cadence. Nothing here touches the DOM, the stream or `$state`.
 */
import type { LogEntry } from '$lib/frame-log';
import type { TraceSource } from '$lib/api-types';

/**
 * Runs a publish once, later, and returns a cancel. A timer in a browser; an immediate call in a
 * test.
 */
export type Schedule = (run: () => void) => () => void;

/**
 * How long arrivals pile up before the screen is redrawn.
 *
 * 100 ms rather than every animation frame. Publishing per frame rebuilds a 1000-element array and
 * its row keys 60 times a second, which causes visible garbage-collection pauses. Ten redraws a
 * second is still faster than anyone can read a scrolling log.
 */
const PUBLISH_INTERVAL_MS = 100;

/** The real scheduler. `FrameInspector.test.ts` injects a synchronous one in its place. */
export const atPublishInterval: Schedule = (run) => {
  const timer = setTimeout(run, PUBLISH_INTERVAL_MS);
  return () => {
    clearTimeout(timer);
  };
};

/** What each source group is, in the operator's terms rather than the crate's. */
export const SOURCE_LABELS: Record<TraceSource, string> = {
  homeconnect: 'home connect',
  mqtt: 'mqtt',
  http: 'http',
  daemon: 'daemon',
  other: 'other',
};

/** The hue (OKLCH degrees) each source is drawn in: its chip dot and its row's rail. */
export const SOURCE_HUES: Record<TraceSource, number> = {
  homeconnect: 255,
  mqtt: 175,
  http: 300,
  daemon: 70,
  other: 250,
};

/** `trace — everything`, `warn and worse`. */
export function levelLabel(level: string): string {
  return `${level}${level === 'trace' ? ' — everything' : ' and worse'}`;
}

/**
 * A stable key per row: a frame is keyed by its seq, a gap by the hole it describes, negated.
 *
 * A number rather than a string: Svelte evaluates this for every row on every publish, and a string
 * key would allocate one per row per redraw.
 *
 * `seq` is a positive counter and a gap's `before` is the seq after the hole, so both are positive;
 * negating the gap key means a frame and a gap can never collide.
 */
export function keyOf(entry: LogEntry): number {
  return entry.row === 'frame' ? entry.frame.seq : -entry.before;
}

/** Severity → the level chip's tone. An unknown level is neutral rather than alarming. */
export function levelClass(severity: string): string {
  switch (severity.toLowerCase()) {
    case 'error': {
      return 'border-destructive/40 bg-destructive/10 text-destructive';
    }
    case 'warn': {
      return 'border-warning/40 bg-warning/10 text-warning';
    }
    case 'debug':
    case 'trace': {
      return 'border-border text-muted-foreground';
    }
    default: {
      return 'border-border bg-muted text-foreground';
    }
  }
}

function pad(n: number, width = 2): string {
  return String(n).padStart(width, '0');
}

/** A frame's server time as `14:03:27.418`, in the browser's time zone. */
export function frameTime(timestampMs: number): string {
  const at = new Date(timestampMs);
  return `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}.${pad(at.getMilliseconds(), 3)}`;
}
