import { describe, expect, it, vi } from 'vitest';
import type { EntityDescriptor, ProgramDescriptor, WriteOutcome } from '$lib/api-types';
import { click, render, select, toastText } from '$lib/dom-testing';
import { Toaster } from '$lib/components/ui/sonner';
import ProgrammeCard from './ProgrammeCard.svelte';

/**
 * The programme card's controls: a programme dropdown, Start and Stop.
 *
 * Each is proposed as a write to a programme pointer, so the confirmation shows exactly what changes:
 * the dropdown writes `SelectedProgram`, Start sets `ActiveProgram` to the programme it shows, Stop sets
 * it to `0`. Like every other write, they ask first on an appliance that produces heat and apply at once
 * elsewhere; the appliance's answer is a toast. Without a `write` callback the card is read-only.
 */
const ACCEPTED: WriteOutcome = { outcome: 'accepted' };
const ECO = 'Dishcare.Dishwasher.Program.Eco50';
const GLASS = 'Dishcare.Dishwasher.Program.Glas40';
const ACTIVE = 'BSH.Common.Root.ActiveProgram';
const SELECTED = 'BSH.Common.Root.SelectedProgram';

const refuse = (): Promise<WriteOutcome> =>
  Promise.reject(new Error('this test must not send a write'));

function pointer(suffix: string, value: string | null): EntityDescriptor {
  return {
    uid: suffix === 'ActiveProgram' ? 256 : 257,
    feature_key: `BSH.Common.Root.${suffix}`,
    feature_path: `BSH/Common/Root/${suffix}`,
    access: 'readWrite',
    writable: true,
    options: [ECO, GLASS],
    label: suffix,
    label_source: 'prefix',
    description: null,
    option_labels: {},
    value,
  };
}

function operationState(value: string): EntityDescriptor {
  return {
    uid: 552,
    feature_key: 'BSH.Common.Status.OperationState',
    feature_path: 'BSH/Common/Status/OperationState',
    access: 'read',
    writable: false,
    options: [],
    label: 'OperationState',
    label_source: 'prefix',
    description: null,
    option_labels: {},
    value,
  };
}

function programme(uid: number, key: string, label: string): ProgramDescriptor {
  return {
    uid,
    feature_key: key,
    label,
    label_source: 'prefix',
    description: null,
    execution: null,
  };
}

const PROGRAMS = [programme(8196, ECO, 'Eco 50'), programme(8197, GLASS, 'Glass 40')];

/**
 * The daemon reports a programme pointer as the programme's key when the profile names it, and as
 * the raw uid otherwise (`hc_core::rendered_value`), so the key is the default here.
 */
function mount(
  options: {
    active?: string;
    selected?: string;
    operation?: string;
    role?: string;
    write?: typeof refuse | undefined;
  } = {},
) {
  return render(ProgrammeCard, {
    entities: [
      pointer('ActiveProgram', options.active ?? '0'),
      pointer('SelectedProgram', options.selected ?? ECO),
      ...(options.operation === undefined ? [] : [operationState(options.operation)]),
    ],
    programs: PROGRAMS,
    slug: 'appliance',
    role: options.role ?? 'dishwasher',
    write: options.write,
  });
}

describe('programme_card_starts_the_selected_programme', () => {
  it('starts the selected programme by name at once on an appliance that does not heat', async () => {
    const write = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = mount({ write });

    click(view.query('programme-start'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(write).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledWith('appliance', { feature_key: ACTIVE, value: ECO });
    view.destroy();
    toaster.destroy();
  });

  it('also finds a selection reported as a raw uid', () => {
    const view = mount({ selected: '8196', write: vi.fn(refuse) });
    expect(view.query('programme-start')?.textContent).toContain('Start Eco 50');
    view.destroy();
  });

  it('asks first on an appliance that produces heat', () => {
    const write = vi.fn(refuse);
    const view = mount({ role: 'oven', write });

    click(view.query('programme-start'));

    expect(write).not.toHaveBeenCalled();
    expect(view.queryPortal('write-pending-label')?.textContent).toContain('Start Eco 50');
    expect(view.queryPortal('write-heat-warning')).not.toBeNull();
    view.destroy();
  });

  it('offers no start while a programme is running', () => {
    const view = mount({ active: ECO, write: vi.fn(refuse) });
    expect(view.query('programme-start')).toBeNull();
    view.destroy();
  });
});

describe('programme_card_stops_the_running_programme', () => {
  it('sets the active programme to 0 while one runs', async () => {
    const write = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = mount({ active: ECO, write });

    click(view.query('programme-stop'));

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(write).toHaveBeenCalledWith('appliance', { feature_key: ACTIVE, value: '0' });
    view.destroy();
    toaster.destroy();
  });

  it('asks first on an appliance that produces heat', () => {
    const write = vi.fn(refuse);
    const view = mount({ active: ECO, role: 'oven', write });

    click(view.query('programme-stop'));

    expect(write).not.toHaveBeenCalled();
    expect(view.queryPortal('write-pending-label')?.textContent).toContain('Stop Eco 50');
    view.destroy();
  });

  it('offers no stop when nothing runs', () => {
    const view = mount({ write: vi.fn(refuse) });
    expect(view.query('programme-stop')).toBeNull();
    view.destroy();
  });
});

describe('programme_card_follows_the_operation_state', () => {
  // A dishwasher keeps reporting its last programme as active after an abort, while it is Ready.
  it('offers Start, not Stop, when the appliance is Ready with a programme still reported', () => {
    const view = mount({ active: GLASS, selected: ECO, operation: 'Ready', write: vi.fn(refuse) });
    expect(view.query('programme-stop')).toBeNull();
    expect(view.query('programme-start')?.textContent).toContain('Start Eco 50');
    expect(view.query('programme-name')?.textContent).toContain('Eco 50');
    view.destroy();
  });

  it('offers Stop during a delayed start', () => {
    const view = mount({ active: GLASS, operation: 'DelayedStart', write: vi.fn(refuse) });
    expect(view.query('programme-stop')?.textContent).toContain('Stop Glass 40');
    expect(view.query('programme-start')).toBeNull();
    view.destroy();
  });
});

describe('programme_card_selects_a_programme', () => {
  it('writes the chosen programme key to the selected programme', async () => {
    const write = vi.fn(() => Promise.resolve(ACCEPTED));
    const toaster = render(Toaster, {});
    const view = mount({ write });

    select(view.query('programme-select'), GLASS);

    await vi.waitFor(() => {
      expect(toastText()).toContain('Applied');
    });
    expect(write).toHaveBeenCalledWith('appliance', { feature_key: SELECTED, value: GLASS });
    view.destroy();
    toaster.destroy();
  });

  it('asks first on an appliance that produces heat, where selecting starts', () => {
    const write = vi.fn(refuse);
    const view = mount({ role: 'oven', write });

    select(view.query('programme-select'), GLASS);

    expect(write).not.toHaveBeenCalled();
    expect(view.queryPortal('write-pending-label')?.textContent).toContain('Glass 40');
    view.destroy();
  });
});

describe('programme_card_is_read_only_without_a_writer', () => {
  it('offers no controls', () => {
    const idle = mount({ write: undefined });
    expect(idle.query('programme-select')).toBeNull();
    expect(idle.query('programme-start')).toBeNull();
    idle.destroy();

    const running = mount({ active: ECO, write: undefined });
    expect(running.query('programme-stop')).toBeNull();
    running.destroy();
  });
});
