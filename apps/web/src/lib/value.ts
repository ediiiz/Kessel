/**
 * How a feature's *value* is shown. (`$lib/appliance.ts` covers the appliance's own vocabulary:
 * role classes, connection states, which keys earn an overview tile.)
 *
 * 1. It never invents a fact and never hides one. Every function is pure and falls back to the
 *    value it was given; nothing renders "Unknown".
 * 2. It renders no identifiers. Values arrive already redacted; nothing here un-redacts.
 * 3. It formats only when the value parses as what the rule expects. Not every key ending in
 *    `Time`, `Duration` or `Relative` holds seconds: `ZoneSelectionTime` holds the enum
 *    `Infinite`/`Limited` and `ApplianceDateTime` holds a timestamp. Neither parses as a number,
 *    so neither is mangled.
 *
 * The suffix lists below are cosmetic heuristics drawn from real appliances, not protocol facts;
 * the survey behind them is in `value.test.ts`.
 */

/** What every surface shows where the appliance has reported nothing. */
const NOTHING = '—';

/**
 * Whether a value reads as "on" for the purpose of a tile's emphasis.
 *
 * Only the words the appliance actually uses, and only exact matches after case folding. Anything
 * else — including a number, an empty string and every unrecognised enum member — is not "on". This
 * decides emphasis only; it never decides what gets written.
 */
const ON_WORDS = new Set(['on', 'true', 'open', 'active', 'run', 'running', 'present']);

export function readsAsOn(value: string | null): boolean {
  return value !== null && ON_WORDS.has(value.trim().toLowerCase());
}

/**
 * A percentage for a progress-shaped value, or `null`.
 *
 * `ProgramProgress` arrives as a number rendered to a string. Anything unparseable, negative or
 * above 100 returns `null` rather than being clamped: a bar at 100% because the value was nonsense
 * would be a lie.
 */
export function progressPercent(value: string | null): number | null {
  if (value === null) {
    return null;
  }
  const parsed = Number(value.trim());
  return Number.isFinite(parsed) && parsed >= 0 && parsed <= 100 ? parsed : null;
}

/**
 * Seconds rendered as `2 h 10 min`, or `null` if the value is not a count of seconds.
 *
 * Home Connect's `RemainingProgramTime` is in seconds; the official app shows hours and minutes,
 * which is the one place this UI reformats a value rather than showing it verbatim. The raw value
 * stays visible in the feature table, so nothing is lost.
 */
export function durationLabel(value: string | null): string | null {
  if (value === null) {
    return null;
  }
  const seconds = Number(value.trim());
  if (!Number.isFinite(seconds) || seconds < 0) {
    return null;
  }
  const total = Math.round(seconds / 60);
  const hours = Math.floor(total / 60);
  const minutes = total % 60;
  if (hours === 0) {
    return `${String(minutes)} min`;
  }
  return minutes === 0 ? `${String(hours)} h` : `${String(hours)} h ${String(minutes)} min`;
}

/**
 * The three ways "nothing reported" arrives, all of which must render as one em dash.
 *
 * JSON `null`, the literal string `"null"`, and the empty string. Together they are most of a real
 * catalogue. The literal string is the one a `value ?? '—'` fallback misses; it appears for keys
 * the appliance has not set (`Duration`, `CleaningLevel`, `ActiveProgram`), so it is treated as
 * absence, not as data.
 */
function isAbsent(value: string | null): boolean {
  if (value === null) {
    return true;
  }
  const trimmed = value.trim();
  return trimmed === '' || trimmed.toLowerCase() === 'null';
}

/**
 * A value with absence corrected, everything else verbatim.
 *
 * For where an operator reads what a feature actually holds before writing to it, so this fixes
 * only absence and deliberately does not reformat numbers, booleans, durations or list blobs the
 * way {@link displayValue} does.
 */
export function blankValue(value: string | null): string {
  return isAbsent(value) ? NOTHING : (value ?? NOTHING);
}

/** Key suffixes that hold a count of seconds. Not every `…Time` key does; see rule 3 above. */
const SECONDS_SUFFIXES = [
  'RemainingProgramTime',
  'ElapsedProgramTime',
  'StartInRelative',
  'EstimatedRemainingTime',
  'SilenceOnDemandRemainingTime',
] as const;

/** Key suffixes where `0` means "no programme", not the number zero. */
const PROGRAMME_SUFFIXES = ['ActiveProgram', 'SelectedProgram'] as const;

/** The parts of a programme descriptor needed to name a programme value. */
export interface ProgramName {
  readonly uid: number;
  readonly feature_key: string | null;
  readonly label: string;
}

/** The name of the programme a value refers to, by uid or by dotted key; `null` if none matches. */
export function programName(value: string, programs: readonly ProgramName[]): string | null {
  const match = programs.find(
    (program) => String(program.uid) === value || program.feature_key === value,
  );
  return match?.label ?? null;
}

/** `{"length":n,"list":[…]}`, the shape the appliance's list values arrive in. */
interface ListBlob {
  readonly length: number;
}

/**
 * `n` if `value` is one of the appliance's list blobs, otherwise `null`.
 *
 * The count is read out of the payload's own `length` field rather than derived from the array, so
 * this summarises rather than interprets. Anything that is not parseable JSON with a non-negative
 * integer `length` falls through and is shown verbatim.
 */
function listLength(value: string): number | null {
  if (!value.startsWith('{')) {
    return null;
  }
  try {
    const parsed: unknown = JSON.parse(value);
    const length = (parsed as ListBlob | null)?.length;
    return typeof length === 'number' && Number.isInteger(length) && length >= 0 ? length : null;
  } catch {
    // Not JSON. The daemon renders every value to a string, so this is reachable and unremarkable.
    return null;
  }
}

/**
 * How a feature's value is shown on the overview tiles.
 *
 * Obeys rule 3 in the module header: each branch below formats only if the value parses as what that
 * branch expects, and otherwise falls through to the value itself. Nothing here is ever `"Unknown"`
 * — an unrecognised value *is* the value.
 */
export function displayValue(
  featureKey: string | null,
  value: string | null,
  programs: readonly ProgramName[] = [],
): string {
  if (isAbsent(value) || value === null) {
    return NOTHING;
  }
  const raw = value.trim();
  const key = featureKey ?? '';

  const folded = raw.toLowerCase();
  if (folded === 'true' || folded === 'false') {
    return folded === 'true' ? 'Yes' : 'No';
  }

  if (PROGRAMME_SUFFIXES.some((suffix) => key.endsWith(suffix))) {
    return raw === '0' ? 'None' : (programName(raw, programs) ?? raw);
  }

  if (SECONDS_SUFFIXES.some((suffix) => key.endsWith(suffix))) {
    // `null` here means "not a count of seconds", which is why the fallback is the raw value.
    return durationLabel(raw) ?? raw;
  }

  if (key.endsWith('ProgramProgress')) {
    const percent = progressPercent(raw);
    return percent === null ? raw : `${String(percent)}%`;
  }

  const items = listLength(raw);
  if (items !== null) {
    return items === 0 ? 'no items' : `${String(items)} item${items === 1 ? '' : 's'}`;
  }

  return raw;
}
