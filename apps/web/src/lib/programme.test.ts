import { describe, expect, it, vi } from 'vitest';
import type { WriteOutcome } from '$lib/api-types';
import { ACTIVE_PROGRAM, SELECTED_PROGRAM, programmeWriter } from './programme';

/**
 * The programme card proposes every action as a write to a programme pointer, so the confirmation
 * shows exactly what changes. This routes each one to the daemon route that carries it.
 */
const ACCEPTED: WriteOutcome = { outcome: 'accepted' };
const ECO = 'Dishcare.Dishwasher.Program.Eco50';

function fakeApi() {
  return {
    write: vi.fn(() => Promise.resolve(ACCEPTED)),
    startProgram: vi.fn(() => Promise.resolve(ACCEPTED)),
    stopProgram: vi.fn(() => Promise.resolve(ACCEPTED)),
  };
}

describe('programme_writer_routes_each_action', () => {
  it('selects through the feature write', async () => {
    const api = fakeApi();
    await programmeWriter(api)('dw', { feature_key: SELECTED_PROGRAM, value: ECO });
    expect(api.write).toHaveBeenCalledWith('dw', { feature_key: SELECTED_PROGRAM, value: ECO });
    expect(api.startProgram).not.toHaveBeenCalled();
  });

  it('starts a named programme through the start route', async () => {
    const api = fakeApi();
    await programmeWriter(api)('dw', { feature_key: ACTIVE_PROGRAM, value: ECO });
    expect(api.startProgram).toHaveBeenCalledWith('dw', { program: ECO });
    expect(api.write).not.toHaveBeenCalled();
  });

  it('stops through the stop route when the active programme is set to 0', async () => {
    const api = fakeApi();
    await programmeWriter(api)('dw', { feature_key: ACTIVE_PROGRAM, value: '0' });
    expect(api.stopProgram).toHaveBeenCalledWith('dw');
    expect(api.startProgram).not.toHaveBeenCalled();
  });

  it('passes any other feature through unchanged', async () => {
    const api = fakeApi();
    await programmeWriter(api)('dw', { feature_key: 'Some.Feature', value: '1' });
    expect(api.write).toHaveBeenCalledWith('dw', { feature_key: 'Some.Feature', value: '1' });
  });
});
