/**
 * The presentation helpers in `$lib/appliance.ts`.
 *
 * Most display names arrive resolved on `EntityDescriptor.label` from the daemon. What is tested here
 * is the sparse-map lookup for enum members and the two helpers that turn a brand and a model
 * designation into something an operator can read and act on.
 */

import { describe, expect, it } from 'vitest';
import {
  connectionDetail,
  contributionPath,
  labelForFeatureKey,
  modelLine,
  optionLabel,
} from './appliance';

describe('optionLabel', () => {
  it('prefers a contributed name for the member', () => {
    const labels = {
      LastProgram: 'Whatever ran last',
      EcoAsDefault: 'Always Eco',
    };
    expect(optionLabel(labels, 'LastProgram')).toBe('Whatever ran last');
    expect(optionLabel(labels, 'EcoAsDefault')).toBe('Always Eco');
  });

  it('falls back to the raw member, which is the normal path', () => {
    // The map is sparse by design: the daemon omits any member whose only name would be derived,
    // because the raw member already reads correctly. `Off` and `On` are the common case.
    expect(optionLabel({}, 'Off')).toBe('Off');
    expect(optionLabel({ Off: 'Nothing' }, 'On')).toBe('On');
  });

  it('spells out a CamelCase member nobody named', () => {
    expect(optionLabel({}, 'SomeMemberNobodyNamed')).toBe('Some member nobody named');
    expect(optionLabel({}, 'ColdWater')).toBe('Cold water');
    expect(optionLabel({}, '24Hours')).toBe('24 hours');
    expect(optionLabel({}, 'ECOMode')).toBe('ECOMode');
  });
});

describe('modelLine', () => {
  it('names the appliance in one line', () => {
    expect(modelLine({ brand: 'BOSCH', vib: 'SMD8TCX01E' })).toBe('BOSCH SMD8TCX01E');
  });

  it('renders whichever half it has, and nothing when it has neither', () => {
    // A stray leading space or a lone heading is worse than showing nothing at all.
    expect(modelLine({ brand: 'BOSCH', vib: '' })).toBe('BOSCH');
    expect(modelLine({ brand: '', vib: 'SMD8TCX01E' })).toBe('SMD8TCX01E');
    expect(modelLine({ brand: '', vib: '' })).toBeNull();
  });
});

describe('contributionPath', () => {
  it('tells the operator exactly which file to create', () => {
    expect(contributionPath({ brand: 'BOSCH', vib: 'SMD8TCX01E' })).toBe(
      'models/BOSCH/SMD8TCX01E.toml',
    );
  });

  it('upper-cases the brand, because the loader keys on it case-insensitively', () => {
    expect(contributionPath({ brand: 'bosch', vib: 'smd8tcx01e' })).toBe(
      'models/BOSCH/SMD8TCX01E.toml',
    );
  });

  it('is null rather than a path with a hole in it', () => {
    expect(contributionPath({ brand: 'BOSCH', vib: '' })).toBeNull();
    expect(contributionPath({ brand: '', vib: 'SMD8TCX01E' })).toBeNull();
  });
});

describe('labelForFeatureKey', () => {
  it('still splits a key for showing the raw form beside the friendly name', () => {
    expect(labelForFeatureKey('Dishcare.Dishwasher.Setting.TimeLight')).toBe('Setting.TimeLight');
  });
});

describe('connectionDetail', () => {
  it("prefers the appliance's own reason over the generic hint", () => {
    // The string the daemon puts on `DeviceSummary.last_failure` — already host-redacted.
    const reason =
      'device offline: the connect or handshake failed: transport closed / no more frames';
    expect(connectionDetail('offline', reason)).toBe(reason);
  });

  it('falls back to the hint when the daemon has no reason to give', () => {
    expect(connectionDetail('offline', null)).toBe('Not connected');
  });

  it('says nothing extra about a healthy link', () => {
    expect(connectionDetail('connected', null)).toBeNull();
  });

  it('still reports a reason that arrives on a non-offline state', () => {
    // `last_failure` survives until the next successful connect, so a retry in flight still has one.
    expect(connectionDetail('connecting', 'refused')).toBe('refused');
  });
});
