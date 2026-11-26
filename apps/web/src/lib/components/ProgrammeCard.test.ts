import { describe, expect, it, vi } from 'vitest';
import type { EntityDescriptor, ProgramDescriptor, WriteOutcome } from '$lib/api-types';
import { click, render, toastText } from '$lib/dom-testing';
import { Toaster } from '$lib/components/ui/sonner';
import ProgrammeCard from './ProgrammeCard.svelte';

/**
 * The programme card's Start control.
 *
 * It starts the programme the appliance reports as selected, naming it, so what is sent is what is
 * shown. Like every other write, it asks first on an appliance that produces heat and applies at once
 * elsewhere. It is offered whenever a selected programme is known and nothing is running; whether the
 * programme starts is the appliance's answer, reported as a toast.
 */
const ACCEPTED: WriteOutcome = { outcome: 'accepted' };
const ECO = 'Dishcare.Dishwasher.Program.Eco50';

const refuse = (): Promise<WriteOutcome> =>
  Promise.reject(new Error('this test must not send a start'));

function pointer(suffix: string, value: string | null): EntityDescriptor {
  return {
    uid: suffix === 'ActiveProgram' ? 256 : 257,
    feature_key: `BSH.Common.Root.${suffix}`,
    feature_path: `BSH/Common/Root/${suffix}`,
    access: 'readWrite',
    writable: true,
    options: [ECO],
    label: suffix,
    label_source: 'prefix',
    description: null,
    option_labels: {},
    value,
  };
}

const PROGRAMS: ProgramDescriptor[] = [
  {
    uid: 8196,
    feature_key: ECO,
    label: 'Eco 50',
    label_source: 'prefix',
    description: null,
    execution: null,
  },
];

/**
 * The daemon reports a programme pointer as the programme's key when the profile names it, and as
 * the raw uid otherwise (`hc_core::rendered_value`), so the key is the default here.
 */
function mount(
  options: {
    active?: string;
    selected?: string;
    role?: string;
    start?: typeof refuse | undefined;
  } = {},
) {
  return render(ProgrammeCard, {
    entities: [
      pointer('ActiveProgram', options.active ?? '0'),
      pointer('SelectedProgram', options.selected ?? ECO),
    ],
    programs: PROGRAMS,
    slug: 'appliance',
    role: options.role ?? 'dishwasher',
    start: options.start,
  });
}

describe('programme_card_starts_the_selected_programme', () => {
  it('starts the selected programme by name at once on an appliance that does not heat', async () => {
    const start = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = mount({ start });

    click(view.query('programme-start'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(start).toHaveBeenCalledTimes(1);
    expect(start).toHaveBeenCalledWith('appliance', {
      feature_key: 'BSH.Common.Root.ActiveProgram',
      value: ECO,
    });
    view.destroy();
    toaster.destroy();
  });

  it('also finds a selection reported as a raw uid', () => {
    const view = mount({ selected: '8196', start: vi.fn(refuse) });
    expect(view.query('programme-start')?.textContent).toContain('Start Eco 50');
    view.destroy();
  });

  it('asks first on an appliance that produces heat', () => {
    const start = vi.fn(refuse);
    const view = mount({ role: 'oven', start });

    click(view.query('programme-start'));

    expect(start).not.toHaveBeenCalled();
    expect(view.queryPortal('write-pending-label')?.textContent).toContain('Start Eco 50');
    expect(view.queryPortal('write-heat-warning')).not.toBeNull();
    view.destroy();
  });

  it('offers no start while a programme is running, or when it cannot send', () => {
    const running = mount({ active: ECO, start: vi.fn(refuse) });
    expect(running.query('programme-start')).toBeNull();
    running.destroy();

    const readOnly = mount({ start: undefined });
    expect(readOnly.query('programme-start')).toBeNull();
    readOnly.destroy();
  });
});
