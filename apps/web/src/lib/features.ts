/** Grouping for the feature browser. */
/** The sections the feature browser groups by, in the order it shows them. */
export const FEATURE_SECTIONS = [
  'Settings',
  'Options',
  'Status',
  'Programmes',
  'Commands',
  'Events',
  'Other',
] as const;

export type FeatureSection = (typeof FEATURE_SECTIONS)[number];

const SECTION_SEGMENTS: Readonly<Record<string, FeatureSection>> = {
  Setting: 'Settings',
  Option: 'Options',
  Status: 'Status',
  Program: 'Programmes',
  Root: 'Programmes',
  Command: 'Commands',
  Event: 'Events',
};

/**
 * Which section a key belongs to, from its kind segment: `Dishcare.Dishwasher.Setting.TimeLight` is
 * a setting. `Root.ActiveProgram` and `Root.SelectedProgram` are programmes. A key with no kind segment, or
 * none at all, is Other.
 */
export function featureSection(featureKey: string | null): FeatureSection {
  for (const segment of featureKey?.split('.') ?? []) {
    const section = SECTION_SEGMENTS[segment];
    if (section !== undefined) {
      return section;
    }
  }
  return 'Other';
}

/** The appliance's American spellings, as the server's catalogue spells them. */
const RESPELLINGS: Readonly<Record<string, string>> = {
  program: 'programme',
  programs: 'programmes',
};

function respell(word: string): string {
  const british = RESPELLINGS[word.toLowerCase()];
  if (british === undefined) {
    return word;
  }
  return /^[A-Z]/u.test(word) ? `${british.charAt(0).toUpperCase()}${british.slice(1)}` : british;
}

/**
 * `ColdWater` → `Cold water`, `24Hours` → `24 hours`, `LastProgram` → `Last programme`. A single
 * word keeps its case.
 */
export function spellMember(member: string): string {
  const words = member
    .replaceAll(/([a-z])([A-Z0-9])/gu, '$1 $2')
    .replaceAll(/([0-9])([A-Za-z])/gu, '$1 $2')
    .split(' ')
    .map((word) => respell(word));
  if (words.length < 2) {
    return words.join(' ');
  }
  return words
    .map((word, at) => (at === 0 || /[A-Z]{2}/u.test(word) ? word : word.toLowerCase()))
    .join(' ');
}

/**
 * What tells a feature apart from a namesake: the key's segments between its kind and its last,
 * spelled out. `…Setting.LearningDishwasher.Feedback.Cleaning` → `Learning dishwasher · Feedback`.
 * Empty when there is nothing between them.
 */
export function featureContext(featureKey: string | null): string {
  const segments = featureKey?.split('.') ?? [];
  const kind = segments.findIndex((segment) => SECTION_SEGMENTS[segment] !== undefined);
  if (kind < 0) {
    return '';
  }
  return segments
    .slice(kind + 1, -1)
    .map((segment) => spellMember(segment))
    .join(' · ');
}

/**
 * The second line for each feature whose name is not unique among `entities`: its context, or its
 * last key segment when the context does not separate it either. Unique names get none.
 */
export function namesakeHints(
  entities: readonly { uid: number; label: string; feature_key: string | null }[],
): ReadonlyMap<number, string> {
  const byLabel = new Map<string, (typeof entities)[number][]>();
  for (const entity of entities) {
    byLabel.set(entity.label, [...(byLabel.get(entity.label) ?? []), entity]);
  }
  const hints = new Map<number, string>();
  for (const group of byLabel.values()) {
    if (group.length < 2) {
      continue;
    }
    const contexts = group.map((entity) => featureContext(entity.feature_key));
    group.forEach((entity, at) => {
      const context = contexts[at] ?? '';
      const unique = context !== '' && contexts.filter((other) => other === context).length === 1;
      const last = entity.feature_key?.split('.').at(-1);
      const hint = unique ? context : last === undefined ? '' : spellMember(last);
      if (hint !== '' && hint !== entity.label) {
        hints.set(entity.uid, hint);
      }
    });
  }
  return hints;
}
