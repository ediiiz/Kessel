/**
 * Presentation only: how the daemon's vocabulary is shown.
 *
 * Maps role classes, connection states and dotted feature keys onto labels, icons and tones, kept
 * separate from `$lib/api.ts` so the wire shapes stay untouched and everything cosmetic is in one
 * place. How a feature's *value* is shown lives in `$lib/value.ts`.
 *
 * 1. It never invents or hides a fact. Every helper is a pure function of what the API sent, with an
 *    identity-ish fallback: an unrecognised role keeps its own word, an unrecognised feature key keeps
 *    its full dotted form. Rendering "Unknown" for a role the daemon named would be worse.
 * 2. It renders no secret identifiers. `DeviceSummary` carries no haId, serial, MAC, psk or iv. The
 *    brand and model designation (`vib`) are not private and are rendered, partly because the
 *    contributed name tables are keyed by that pair (see {@link contributionPath}).
 *
 * Most names arrive already resolved: `EntityDescriptor.label` and `ProgramDescriptor.label` come from
 * the daemon, which layers a contributed per-model table over a per-prefix one over a spelling
 * transform of the identifier. This module keeps the shape helpers: {@link labelForFeatureKey} for
 * showing the raw key beside the friendly name, and {@link optionLabel} for the sparse member-name map.
 *
 * {@link OVERVIEW_KEYS} and {@link labelForFeatureKey} pattern-match on Home Connect feature keys
 * (`BSH.Common.Status.OperationState` and friends). These are cosmetic heuristics, not protocol facts:
 * nothing behavioural depends on them. A key that matches nothing still appears in the full feature
 * table, and the overview simply shows fewer tiles.
 */

/** A tone, resolved to Tailwind classes by the components. Not a colour — a meaning. */
import { spellMember } from './features';

export type Tone = 'live' | 'working' | 'idle' | 'bad';

export interface ConnectionLook {
  /** The daemon's own word, unchanged. Rendered as-is so the raw state is always readable. */
  readonly state: string;
  /** Prose for an operator who does not know the state machine. */
  readonly hint: string;
  readonly tone: Tone;
}

const CONNECTION: Record<string, ConnectionLook> = {
  connected: {
    state: 'connected',
    hint: 'Live — the appliance is reporting',
    tone: 'live',
  },
  handshake: {
    state: 'handshake',
    hint: 'Completing the handshake',
    tone: 'working',
  },
  connecting: {
    state: 'connecting',
    hint: 'Opening the socket',
    tone: 'working',
  },
  reconnecting: {
    state: 'reconnecting',
    hint: 'The link dropped; retrying',
    tone: 'bad',
  },
  offline: { state: 'offline', hint: 'Not connected', tone: 'idle' },
};

/**
 * How to show a connection state.
 *
 * An unrecognised state keeps its own word and is shown as `idle` rather than as an error: the
 * daemon's `DeviceConnectionState` can grow a variant without this file being edited, and a new
 * state is not the same thing as a failure.
 */
export function connectionLook(state: string): ConnectionLook {
  return CONNECTION[state] ?? { state, hint: state, tone: 'idle' };
}

/**
 * The line shown beside a connection state — the appliance's own reason when there is one.
 *
 * `offline` covers both a switched-off appliance and one that is answering every dial and rejecting
 * the handshake, and only the daemon knows which. `last_failure` carries its word; the generic hint
 * is the fallback for when it has none.
 *
 * Returns `null` when there is nothing worth adding, which is a healthy link.
 */
export function connectionDetail(state: string, lastFailure: string | null): string | null {
  if (lastFailure !== null && lastFailure !== '') {
    return lastFailure;
  }
  const look = connectionLook(state);
  return look.tone === 'live' ? null : look.hint;
}

/**
 * A human label for a role class.
 *
 * `fridgefreezer` is one word on the wire and two in English, which is the only reason this map is
 * not just a capitalisation. An unknown role is capitalised and returned.
 */
const ROLE_LABELS: Record<string, string> = {
  cooktop: 'Hob',
  hob: 'Hob',
  oven: 'Oven',
  dishwasher: 'Dishwasher',
  fridgefreezer: 'Fridge freezer',
  refrigerator: 'Fridge',
  freezer: 'Freezer',
  washer: 'Washing machine',
  washingmachine: 'Washing machine',
  dryer: 'Tumble dryer',
  coffeemaker: 'Coffee machine',
  coffeemachine: 'Coffee machine',
  hood: 'Extractor hood',
  warmingdrawer: 'Warming drawer',
  winecooler: 'Wine cooler',
  dishwarmer: 'Dish warmer',
};

export function roleLabel(role: string): string {
  return ROLE_LABELS[role.toLowerCase()] ?? role.charAt(0).toUpperCase() + role.slice(1);
}

/**
 * Which roles are heat-producing, and therefore get a standing caution on their card.
 *
 * This is presentation, not a gate: the daemon does not refuse writes to a hob or an oven, so the UI
 * makes it obvious which appliance a write is about to reach. Anything not listed here is treated as
 * non-heating, the less cautious default, so a new heating role must be added here deliberately.
 */
const HEATING_ROLES = new Set([
  'cooktop',
  'hob',
  'oven',
  'warmingdrawer',
  'dishwarmer',
  'coffeemaker',
  'coffeemachine',
]);

export function isHeating(role: string): boolean {
  return HEATING_ROLES.has(role.toLowerCase());
}

/** The hue (OKLCH degrees) a role's artwork and glow are tinted with. */
const ROLE_HUES: Record<string, number> = {
  dishwasher: 232,
  fridgefreezer: 205,
  refrigerator: 205,
  freezer: 220,
  winecooler: 350,
  oven: 50,
  cooktop: 32,
  hob: 32,
  washer: 265,
  washingmachine: 265,
  dryer: 295,
  coffeemaker: 60,
  coffeemachine: 60,
  hood: 180,
  warmingdrawer: 40,
  dishwarmer: 40,
};

export function roleHue(role: string): number {
  return ROLE_HUES[role.toLowerCase()] ?? 255;
}

/**
 * The feature keys the overview section promotes to a tile, in the order they should appear.
 *
 * Matched by **suffix** against the dotted key, so `BSH.Common.Status.OperationState` is found by
 * `Status.OperationState` regardless of the vendor prefix (`BSH.Common`, `Dishcare.Dishwasher`,
 * `Cooking.Oven`, …), which differs per appliance. First match wins, so the order here is the
 * display order.
 *
 * Heuristic; see this module's header.
 */
export const OVERVIEW_KEYS: readonly {
  readonly suffix: string;
  readonly label: string;
  /** Shown instead of the value when it is `0`, where zero means the feature is not in use. */
  readonly zero?: string;
}[] = [
  { suffix: 'Setting.PowerState', label: 'Power' },
  { suffix: 'Status.OperationState', label: 'Operation' },
  { suffix: 'Root.ActiveProgram', label: 'Active programme' },
  { suffix: 'Root.SelectedProgram', label: 'Selected programme' },
  { suffix: 'Option.ProgramProgress', label: 'Progress' },
  { suffix: 'Option.RemainingProgramTime', label: 'Time remaining' },
  { suffix: 'Option.StartInRelative', label: 'Delayed start', zero: 'Off' },
  { suffix: 'Status.DoorState', label: 'Door' },
  { suffix: 'Status.RemoteControlStartAllowed', label: 'Remote start' },
  { suffix: 'Status.RemoteControlActive', label: 'Remote control' },
  { suffix: 'Status.LocalControlActive', label: 'Local control' },
  { suffix: 'Setting.TemperatureUnit', label: 'Temperature unit' },
];

/**
 * A short label for any feature key: its last two dotted segments, which is where Home Connect keys
 * carry their meaning (`…Option.IntensivZone` → `Option.IntensivZone`).
 *
 * The full key is never thrown away — the callers render this as the headline and keep the dotted key
 * beside or beneath it — because the full key is what an operator types into a write and what the
 * MQTT topic derives from.
 */
export function labelForFeatureKey(featureKey: string | null): string {
  if (featureKey === null) {
    return '(unnamed)';
  }
  const segments = featureKey.split('.');
  return segments.length <= 2 ? featureKey : segments.slice(-2).join('.');
}

/**
 * The display name for one enum member, or the raw member when nobody has named it.
 *
 * `option_labels` is deliberately **sparse**: the daemon omits any member whose only available name
 * would be the derived spelling, because the raw member is already a perfectly good thing to render
 * and shipping `On` → `"On"` for every unnamed member would inflate the payload to say nothing. So the
 * `??` here is the normal path, not an error path.
 *
 * The argument is the **raw** member — the string a write carries — and it stays the raw member in
 * `data-value`. Only the visible text changes.
 */
export function optionLabel(labels: Record<string, string>, member: string): string {
  return labels[member] ?? spellMember(member);
}

/**
 * How to name the appliance in one line: `BOSCH SMD8TCX01E`.
 *
 * `null` when the profile declared neither, which keeps the caller from rendering a stray space or an
 * empty heading. A profile carrying only one of the two renders that one.
 */
export function modelLine(device: { brand: string; vib: string }): string | null {
  const parts = [device.brand, device.vib].map((part) => part.trim()).filter((part) => part !== '');
  return parts.length === 0 ? null : parts.join(' ');
}

/**
 * The contributed-name file this appliance would be described by:
 * `models/BOSCH/SMD8TCX01E.toml`, relative to `apps/server/data/labels/`.
 *
 * Shown as the model line's hover text, so an operator who wants better names for their appliance is
 * given the exact path to create. `null` when either half is missing, because a path with a hole in it
 * is worse than no path.
 */
export function contributionPath(device: { brand: string; vib: string }): string | null {
  const brand = device.brand.trim().toUpperCase();
  const vib = device.vib.trim().toUpperCase();
  // Both halves or nothing: `models//SMD8TCX01E.toml` would be a path the loader rejects, offered to
  // an operator as if it were the answer.
  if (brand === '' || vib === '') {
    return null;
  }
  return `models/${brand}/${vib}.toml`;
}
