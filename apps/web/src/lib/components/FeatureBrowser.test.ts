import { describe, expect, it } from 'vitest';
import type { EntityDescriptor } from '$lib/api-types';
import { click, render, type } from '$lib/dom-testing';
import FeatureBrowser from './FeatureBrowser.svelte';

function entity(overrides: Partial<EntityDescriptor> & { uid: number }): EntityDescriptor {
  return {
    feature_key: null,
    feature_path: null,
    access: 'read',
    writable: false,
    options: [],
    label: `Feature ${String(overrides.uid)}`,
    label_source: 'derived',
    description: null,
    option_labels: {},
    value: null,
    ...overrides,
  };
}

const ENTITIES: EntityDescriptor[] = [
  entity({
    uid: 512,
    feature_key: 'BSH.Common.Status.OperationState',
    label: 'Operation state',
    value: 'Ready',
  }),
  entity({
    uid: 513,
    feature_key: 'Dishcare.Dishwasher.Option.IntensivZone',
    access: 'readWrite',
    writable: true,
    options: ['On', 'Off'],
    label: 'Intensive zone',
    value: 'Off',
  }),
  entity({
    uid: 514,
    feature_key: 'BSH.Common.Setting.ChildLock',
    access: 'readWrite',
    writable: true,
    options: ['On', 'Off'],
    label: 'Child lock',
  }),
  // Every optional field null at once: the profile does not always name a feature.
  entity({ uid: 515, access: null, label: 'Unnamed feature 0x0203' }),
];

const never = (): Promise<never> => Promise.reject(new Error('this test must not send a write'));

function browse(write: typeof never | null = never): ReturnType<typeof render> {
  return render(FeatureBrowser, {
    entities: ENTITIES,
    slug: 'dishwasher',
    role: 'dishwasher',
    write,
  });
}

const labels = (view: ReturnType<typeof render>): string[] =>
  view.queryAll('feature-label').map((label) => label.textContent ?? '');

describe('feature_browser_finds_a_feature_without_its_key', () => {
  it('opens on the features that report a value, grouped by section', () => {
    const view = browse();

    expect(labels(view)).toStrictEqual(['Intensive zone', 'Operation state']);
    expect(
      view.queryAll('feature-section').map((section) => section.dataset.section),
    ).toStrictEqual(['Options', 'Status']);
    view.destroy();
  });

  it('shows what can be changed, and everything, on request', () => {
    const view = browse();

    click(view.query('feature-filter-writable'));
    view.flush();
    expect(labels(view)).toStrictEqual(['Child lock', 'Intensive zone']);

    click(view.query('feature-filter-all'));
    view.flush();
    expect(labels(view)).toHaveLength(ENTITIES.length);
    view.destroy();
  });

  it('searches the name, the key and the value', () => {
    const view = browse();
    click(view.query('feature-filter-all'));

    type(view.query('feature-search'), 'intensiv');
    view.flush();
    expect(labels(view)).toStrictEqual(['Intensive zone']);

    type(view.query('feature-search'), 'ready');
    view.flush();
    expect(labels(view)).toStrictEqual(['Operation state']);

    type(view.query('feature-search'), 'nothing like this');
    view.flush();
    expect(view.query('feature-empty')).not.toBeNull();
    view.destroy();
  });
});

describe('feature_browser_offers_a_change_only_where_one_is_possible', () => {
  it('offers the members of a writable enumeration, and the current one is not offered again', () => {
    const view = browse();
    const row = view.container.querySelector('[data-uid="513"]');
    click(row?.querySelector('[data-testid="feature-toggle"]') ?? null);
    view.flush();

    const chips = [
      ...(row?.querySelectorAll<HTMLButtonElement>('[data-testid="feature-option"]') ?? []),
    ];
    expect(chips.map((chip) => chip.dataset.value)).toStrictEqual(['On', 'Off']);
    expect(chips.map((chip) => chip.disabled)).toStrictEqual([false, true]);
    view.destroy();
  });

  it('shows a read-only feature, a keyless one, and any on a never-writable appliance as read only', () => {
    const view = browse();
    click(view.query('feature-filter-all'));
    view.flush();
    for (const uid of ['512', '515']) {
      const row = view.container.querySelector(`[data-uid="${uid}"]`);
      click(row?.querySelector('[data-testid="feature-toggle"]') ?? null);
      view.flush();
      expect(row?.querySelector('[data-testid="feature-readonly"]')).not.toBeNull();
      expect(row?.querySelector('button[data-testid="feature-option"], input')).toBeNull();
    }
    view.destroy();

    const locked = browse(null);
    const row = locked.container.querySelector('[data-uid="513"]');
    click(row?.querySelector('[data-testid="feature-toggle"]') ?? null);
    locked.flush();
    expect(row?.querySelector('[data-testid="feature-option"]')).toBeNull();
    expect(row?.querySelector('[data-testid="feature-readonly"]')?.textContent).toContain(
      'Read only',
    );
    locked.destroy();
  });
});
