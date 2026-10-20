import { describe, expect, it } from 'vitest';
import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
import { render } from '$lib/dom-testing';
import EntityOverview from './EntityOverview.svelte';

function entity(overrides: Partial<EntityDescriptor> = {}): EntityDescriptor {
  return {
    uid: 1,
    feature_key: 'BSH.Common.Root.SelectedProgram',
    feature_path: 'BSH/Common/Root/SelectedProgram',
    access: 'readWrite',
    writable: true,
    options: [],
    label: 'Selected Program',
    label_source: 'prefix',
    description: null,
    option_labels: {},
    value: '8192',
    ...overrides,
  };
}

const ECO: ProgramDescriptor = {
  uid: 8192,
  feature_key: 'Dishcare.Dishwasher.Program.Eco50',
  label: 'Eco 50°',
  label_source: 'prefix',
  description: null,
  execution: 'selectAndStart',
};

describe('the_overview_names_the_selected_programme', () => {
  it('shows the programme name rather than its uid', () => {
    const view = render(EntityOverview, { entities: [entity()], programs: [ECO] });

    const text = view.query('entity-overview')?.textContent ?? '';
    expect(text).toContain('Eco 50°');
    expect(text).not.toContain('8192');
    view.destroy();
  });

  it('falls back to the reported value when no programme matches', () => {
    const view = render(EntityOverview, { entities: [entity({ value: '4242' })], programs: [ECO] });

    expect(view.query('entity-overview')?.textContent).toContain('4242');
    view.destroy();
  });
});

describe('a_zero_delayed_start_reads_as_off', () => {
  const delay = (value: string): EntityDescriptor =>
    entity({
      uid: 2,
      feature_key: 'BSH.Common.Option.StartInRelative',
      feature_path: 'BSH/Common/Option/StartInRelative',
      value,
    });

  it('shows Off rather than 0 min when no delay is set', () => {
    const view = render(EntityOverview, { entities: [delay('0')] });

    const text = view.query('entity-overview')?.textContent ?? '';
    expect(text).toContain('Delayed start');
    expect(text).toContain('Off');
    view.destroy();
  });

  it('shows the delay when one is set', () => {
    const view = render(EntityOverview, { entities: [delay('3600')] });

    expect(view.query('entity-overview')?.textContent).not.toContain('Off');
    view.destroy();
  });
});
