import { describe, expect, it, vi } from 'vitest';
import type { EntityDescriptor, WriteOutcome as Outcome } from '$lib/api-types';
import { click, render, toastText, toastTexts, type } from '$lib/dom-testing';
import { Toaster } from '$lib/components/ui/sonner';
import FeatureBrowser from './FeatureBrowser.svelte';

/**
 * The confirmation gate.
 *
 * The safety property: clicking submit writes zero times, and only confirming writes, exactly once.
 * The daemon carries whatever this component sends to whichever appliance is addressed, so the click
 * that commits has to be a click that says so.
 *
 * The confirmation is a modal dialog, so its contents are portalled to `document.body` and found with
 * `queryPortal`. The outcome and the failure are toasts, read with `toastText()`, which needs a
 * `Toaster` mounted in the same document, exactly as the app shell mounts one.
 */
type WriteOutcome = Outcome;

function feature(
  uid: number,
  key: string,
  value: string | null,
  options: string[] = [],
): EntityDescriptor {
  return {
    uid,
    feature_key: key,
    feature_path: null,
    access: 'readWrite',
    writable: true,
    options,
    label: key.split('.').at(-1) ?? key,
    label_source: 'derived',
    description: null,
    option_labels: {},
    value,
  };
}

const ENTITIES = [
  feature(1, 'BSH.Common.Setting.PowerState', 'Off', ['On', 'Off']),
  feature(2, 'Some.Feature', '0'),
  feature(3, 'Cooking.Hob.Setting.Something', '0'),
];

/** The browser for one appliance, as the device page's sheet mounts it. */
function browser(slug: string, role: string, write: Writer): ReturnType<typeof render> {
  return render(FeatureBrowser, { entities: ENTITIES, slug, role, write });
}

type Writer = (
  slug: string,
  request: { feature_key: string; value: string },
) => Promise<WriteOutcome>;

const ACCEPTED: WriteOutcome = { outcome: 'accepted' };

/** Passed wherever a test asserts that nothing is sent. Calling it fails the test outright. */
const refuse = (): Promise<WriteOutcome> =>
  Promise.reject(new Error('this test must not send a write'));

/** Opens the feature's row and proposes `value`: its chip for an enumeration, else the field. */
function compose(view: ReturnType<typeof render>, key: string, value: string): void {
  const row = view.container.querySelector<HTMLElement>(`[data-key="${key}"]`);
  click(row?.querySelector('[data-testid="feature-toggle"]') ?? null);
  view.flush();
  const chip = row?.querySelector<HTMLElement>(
    `[data-testid="feature-option"][data-value="${value}"]`,
  );
  if (chip !== null && chip !== undefined) {
    click(chip);
    return;
  }
  type(row?.querySelector('[data-testid="feature-input"]') ?? null, value);
  click(row?.querySelector('[data-testid="feature-set"]') ?? null);
}

describe('write_requires_an_explicit_confirmation', () => {
  it('sends nothing when a value is picked — it asks first', () => {
    const write = vi.fn(refuse);
    const view = browser('dishwasher', 'dishwasher', write);

    compose(view, 'BSH.Common.Setting.PowerState', 'On');

    expect(write).not.toHaveBeenCalled();
    // The confirmation has to actually be on screen, or this test would also pass for a component
    // whose submit button does nothing.
    expect(view.queryPortal('write-confirm')).not.toBeNull();
    view.destroy();
  });

  it('names the device, the feature and the value in the confirmation', () => {
    const view = browser('dishwasher', 'dishwasher', refuse);

    compose(view, 'BSH.Common.Setting.PowerState', 'On');

    const pending = view.queryPortal('write-pending')?.textContent ?? '';
    expect(pending).toContain('dishwasher');
    expect(pending).toContain('BSH.Common.Setting.PowerState');
    expect(pending).toContain('On');
    view.destroy();
  });

  it('writes exactly once, with the picked key and value, when the confirmation is accepted', async () => {
    const write = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = browser('dishwasher', 'dishwasher', write);

    compose(view, 'BSH.Common.Setting.PowerState', 'On');
    click(view.queryPortal('write-confirm'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(write).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledWith('dishwasher', {
      feature_key: 'BSH.Common.Setting.PowerState',
      value: 'On',
    });
    view.destroy();
    toaster.destroy();
  });
});

/**
 * Sending and being accepted are two different events, and the operator is told about both. The one
 * that matters is the first: between the confirmation closing and the daemon answering there is a
 * round trip to an appliance, and a UI that says nothing during it looks like a UI that dropped the
 * write.
 *
 * The in-flight toast is *replaced* rather than left stacked — a "sending…" sitting above a
 * "refused" reads as two writes. Hence the count assertions: `toastText()` alone cannot tell those
 * apart.
 */
describe('a_write_is_announced_when_sent_and_again_when_answered', () => {
  it('shows a sending toast in flight, then swaps it for the outcome', async () => {
    // Held open so the in-flight state can be observed, then settled by hand. Definitely assigned:
    // the executor runs synchronously inside `write()`, which `compose` + confirm below both reach
    // before anything reads it.
    let settle!: (outcome: WriteOutcome) => void;
    const toaster = render(Toaster, {});
    const view = browser(
      'dishwasher',
      'dishwasher',
      () =>
        new Promise<WriteOutcome>((resolve) => {
          settle = resolve;
        }),
    );

    compose(view, 'BSH.Common.Setting.PowerState', 'On');
    click(view.queryPortal('write-confirm'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Sending');
    });
    expect(toastText()).not.toContain('Applied');

    settle(ACCEPTED);
    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(toastText()).not.toContain('Sending');
    expect(toastTexts()).toHaveLength(1);
    view.destroy();
    toaster.destroy();
  });
});

/**
 * Its own `describe` to stay within `max-lines-per-function`.
 */
describe('write_outcome_is_reported_only_as_a_toast', () => {
  it('leaves nothing inline under the control once the daemon has answered', async () => {
    const toaster = render(Toaster, {});
    const view = browser('dishwasher', 'dishwasher', () => Promise.resolve(ACCEPTED));

    compose(view, 'BSH.Common.Setting.PowerState', 'On');
    click(view.queryPortal('write-confirm'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(view.query('write-outcome')).toBeNull();
    view.destroy();
    toaster.destroy();
  });
});

/**
 * Its own `describe` to stay within `max-lines-per-function`.
 */
describe('write_can_be_abandoned_without_sending', () => {
  it('sends nothing when the confirmation is dismissed', () => {
    const write = vi.fn(refuse);
    const view = browser('dishwasher', 'dishwasher', write);

    compose(view, 'BSH.Common.Setting.PowerState', 'On');
    click(view.queryPortal('write-cancel'));

    expect(write).not.toHaveBeenCalled();
    expect(view.queryPortal('write-confirm')).toBeNull();
    view.destroy();
  });
});

/**
 * A heat-producing appliance gets a louder confirmation. This is presentation, not a gate (the write
 * is carried either way), but an announced write is only useful if it says which appliance is about
 * to be commanded.
 */
describe('write_confirmation_warns_for_a_heating_appliance', () => {
  it('says the appliance heats when the role is a hob, and does not when it is a dishwasher', () => {
    const hob = browser('cooktop', 'cooktop', refuse);
    compose(hob, 'Cooking.Hob.Setting.Something', '1');
    expect(hob.queryPortal('write-heat-warning')).not.toBeNull();
    hob.destroy();

    const dishwasher = browser('dishwasher', 'dishwasher', refuse);
    compose(dishwasher, 'BSH.Common.Setting.PowerState', 'On');
    expect(dishwasher.queryPortal('write-heat-warning')).toBeNull();
    dishwasher.destroy();
  });
});

/**
 * A `{ outcome: "refused" }` must never be dressed up as success or silently retried.
 */
describe('write_refusal_is_rendered_and_never_retried', () => {
  it("shows the daemon's reason and attempts the write exactly once", async () => {
    const write = vi.fn(() =>
      Promise.resolve<WriteOutcome>({
        outcome: 'refused',
        reason: 'the wire serialization is not implemented yet',
      }),
    );
    const toaster = render(Toaster, {});
    const view = browser('dishwasher', 'dishwasher', write);

    compose(view, 'Some.Feature', '1');
    click(view.queryPortal('write-confirm'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Not applied');
    });
    expect(toastText()).toContain('the wire serialization is not implemented yet');
    expect(write).toHaveBeenCalledTimes(1);
    view.destroy();
    toaster.destroy();
  });
});
