import { describe, expect, it, vi } from 'vitest';
import type { EntityDescriptor, WriteOutcome } from '$lib/api-types';
import { click, render, toastText } from '$lib/dom-testing';
import { Toaster } from '$lib/components/ui/sonner';
import QuickActions from './QuickActions.svelte';

/**
 * The quick-action controls.
 *
 * A tile sends the other declared member when tapped. Until the appliance has reported which member
 * is current, the tile offers both instead of guessing. On an appliance that produces heat a tap only
 * proposes and the confirmation writes; elsewhere it applies at once. The tile always follows what the
 * appliance reports, so a refusal leaves it where the appliance is.
 *
 * Second, which features get a control at all: writable by access, exactly two declared members, and
 * a feature key to send. Widening that would mean inventing a value for something with no safe
 * two-state representation.
 *
 * The confirmation is a modal dialog, so its contents are portalled to `document.body` and found with
 * `queryPortal`. Outcomes are toasts, read with `toastText()`, which needs a `Toaster` in the same
 * document exactly as the app shell mounts one.
 */
const ACCEPTED: WriteOutcome = { outcome: 'accepted' };

/** Passed wherever a test asserts that nothing is sent. Calling it fails the test outright. */
const refuse = (): Promise<WriteOutcome> =>
  Promise.reject(new Error('this test must not send a write'));

function entity(overrides: Partial<EntityDescriptor> = {}): EntityDescriptor {
  return {
    uid: 539,
    feature_key: 'BSH.Common.Setting.PowerState',
    feature_path: 'BSH/Common/Setting/PowerState',
    access: 'readWrite',
    writable: true,
    options: ['Off', 'On'],
    // The resolved display name and the sparse member map. `options` above stays raw, since that is
    // what a write carries; these are the display layer beside it.
    label: 'Power',
    label_source: 'prefix',
    description: null,
    option_labels: {},
    value: 'Off',
    ...overrides,
  };
}

/** The single toggle tile, failing legibly when it is missing. */
function toggle(view: { query: (id: string) => HTMLElement | null }): HTMLElement {
  const found = view.query('quick-action-toggle');
  if (found === null) {
    throw new Error('expected a quick-action toggle');
  }
  return found;
}

/**
 * One option control by index.
 *
 * `noUncheckedIndexedAccess` is on, so indexing `queryAll` yields `HTMLElement | undefined`. Asserting
 * here rather than at each call site keeps the failure legible — "expected an option control at index 1,
 * got 0" beats a type error, and a missing control is exactly the regression this suite watches for.
 */
function option(controls: HTMLElement[], index: number): HTMLElement {
  const found = controls[index];
  if (found === undefined) {
    throw new Error(
      `expected an option control at index ${String(index)}, got ${String(controls.length)}`,
    );
  }
  return found;
}

/** Renders the hob's cooktop role deliberately: it is the appliance the safety rules are about. */
function mount(entities: EntityDescriptor[], write = refuse) {
  return render(QuickActions, { entities, slug: 'cooktop', role: 'cooktop', write });
}

/** An appliance that does not produce heat, where a tap applies straight away. */
function mountDishwasher(entities: EntityDescriptor[], write: typeof refuse) {
  return render(QuickActions, { entities, slug: 'dishwasher', role: 'dishwasher', write });
}

describe('quick_actions_offer_one_control_per_two_state_writable_feature', () => {
  it('renders one tile that will send the other declared member', () => {
    const view = mount([entity()]);

    expect(view.queryAll('quick-action')).toHaveLength(1);
    expect(toggle(view).dataset.value).toBe('On');
    expect(toggle(view).getAttribute('aria-pressed')).toBe('false');
    expect(view.query('quick-action-value')?.textContent).toContain('Off');
    expect(toggle(view).textContent).toContain('Power');
    view.destroy();
  });

  it('offers both members, and guesses neither, before the appliance has reported one', () => {
    const view = mount([entity({ value: null })]);

    expect(view.query('quick-action-toggle')).toBeNull();
    expect(view.queryAll('quick-action-option').map((o) => o.dataset.value)).toStrictEqual([
      'Off',
      'On',
    ]);
    view.destroy();
  });

  it('offers nothing for a feature it cannot represent as two states', () => {
    const view = mount([
      entity({ uid: 1, writable: false }),
      entity({ uid: 2, options: [] }),
      entity({ uid: 3, options: ['Inactive', 'Ready', 'Run'] }),
      entity({ uid: 4, feature_key: null }),
    ]);

    expect(view.queryAll('quick-action')).toHaveLength(0);
    expect(view.query('quick-actions')).toBeNull();
    view.destroy();
  });
});

describe('quick_actions_on_a_heating_appliance_ask_first', () => {
  it('sends nothing when a tile is tapped, and names the change in the confirmation', () => {
    const write = vi.fn(refuse);
    const view = mount([entity()], write);

    click(toggle(view));

    expect(write).not.toHaveBeenCalled();
    expect(view.queryPortal('write-confirm')).not.toBeNull();
    expect(view.queryPortal('write-pending-label')?.textContent).toContain('Power → On');
    expect(view.queryPortal('write-heat-warning')).not.toBeNull();
    view.destroy();
  });

  it('proposes the exact member pressed when the value is not yet known', () => {
    const view = mount([entity({ options: ['IndicationOff', 'IndicationOn'], value: null })]);

    click(option(view.queryAll('quick-action-option'), 0));

    const pending = view.queryPortal('write-pending')?.textContent ?? '';
    expect(pending).toContain('cooktop');
    expect(pending).toContain('BSH.Common.Setting.PowerState');
    expect(pending).toContain('IndicationOff');
    view.destroy();
  });

  it('writes exactly once when the confirmation is accepted', async () => {
    const write = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = mount([entity()], write);

    click(toggle(view));
    click(view.queryPortal('write-confirm'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(write).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledWith('cooktop', {
      feature_key: 'BSH.Common.Setting.PowerState',
      value: 'On',
    });
    view.destroy();
    toaster.destroy();
  });

  it('sends nothing when the confirmation is dismissed', () => {
    const write = vi.fn(refuse);
    const view = mount([entity()], write);

    click(toggle(view));
    click(view.queryPortal('write-cancel'));

    expect(write).not.toHaveBeenCalled();
    expect(view.queryPortal('write-confirm')).toBeNull();
    view.destroy();
  });
});

describe('quick_actions_apply_at_once_on_an_appliance_without_heat', () => {
  it('sends the other member on the first tap, with no confirmation', async () => {
    const write = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = mountDishwasher([entity()], write);

    click(toggle(view));

    expect(view.queryPortal('write-confirm')).toBeNull();
    expect(write).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledWith('dishwasher', {
      feature_key: 'BSH.Common.Setting.PowerState',
      value: 'On',
    });
    await vi.waitFor(() => {
      expect(toastText()).toContain('Power → On');
    });
    view.destroy();
    toaster.destroy();
  });

  it('marks the tile busy and blocks a second tap until the appliance answers', async () => {
    let settle!: (outcome: WriteOutcome) => void;
    const write = vi.fn(
      () =>
        new Promise<WriteOutcome>((resolve) => {
          settle = resolve;
        }),
    );
    const view = mountDishwasher([entity()], write);

    click(toggle(view));
    await vi.waitFor(() => {
      expect(toggle(view).getAttribute('aria-busy')).toBe('true');
    });
    click(toggle(view));
    expect(write).toHaveBeenCalledTimes(1);

    settle(ACCEPTED);
    await vi.waitFor(() => {
      expect(toggle(view).getAttribute('aria-busy')).toBe('false');
    });
    view.destroy();
  });

  it('keeps showing the reported value when the appliance refuses', async () => {
    const write = vi.fn(() =>
      Promise.resolve<WriteOutcome>({ outcome: 'refused', reason: 'the appliance refused it' }),
    );
    const toaster = render(Toaster, {});
    const view = mountDishwasher([entity()], write);

    click(toggle(view));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Not applied: Power → On');
    });
    expect(view.query('quick-action-value')?.textContent).toContain('Off');
    expect(toggle(view).dataset.value).toBe('On');
    view.destroy();
    toaster.destroy();
  });
});
