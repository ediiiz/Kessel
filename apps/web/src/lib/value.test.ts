/**
 * How a feature's value is *shown*.
 *
 * Every case below is a shape real appliances report. A survey of a cooktop, oven, dishwasher and
 * fridge (1075 entities, via `GET /api/devices/{slug}/entities`):
 *
 *   | shape                                  | count |
 *   |----------------------------------------|-------|
 *   | JSON `null`                            |   536 |
 *   | a word or enum member (`Off`, `Level0`) |   262 |
 *   | **the literal four-character `null`**   |   124 |
 *   | `true` / `false`                        |    65 |
 *   | an integer                              |    57 |
 *   | the empty string                        |    31 |
 *   | a JSON blob (`{"length":0,"list":[]}`)  |    35 |
 *
 * The literal `"null"` would show on screen as the word "null", because `value ?? '—'` cannot catch
 * a string. No dotted enum value appeared, so nothing here shortens one.
 */
import { describe, expect, it } from 'vitest';
import { blankValue, displayValue, programName } from './value';

describe('a_value_is_shown_as_reported_unless_it_is_measurably_unreadable', () => {
  it('renders every flavour of "nothing reported" as one em dash', () => {
    // Together these three are most of a real catalogue.
    expect(displayValue('BSH.Common.Root.SelectedProgram', null)).toBe('—');
    expect(displayValue('BSH.Common.Root.SelectedProgram', 'null')).toBe('—');
    expect(displayValue('BSH.Common.Setting.Timezone', '')).toBe('—');
    expect(displayValue('BSH.Common.Setting.Timezone', '   ')).toBe('—');
  });

  it('reads a boolean as an answer rather than as a wire literal', () => {
    expect(displayValue('BSH.Common.Status.RemoteControlActive', 'true')).toBe('Yes');
    expect(displayValue('BSH.Common.Status.LocalControlActive', 'false')).toBe('No');
    // Case-folded, because the wire form is not this module's to guarantee.
    expect(displayValue('BSH.Common.Status.RemoteControlActive', 'True')).toBe('Yes');
  });

  it('says no programme is selected instead of showing a bare zero', () => {
    expect(displayValue('BSH.Common.Root.ActiveProgram', '0')).toBe('None');
    expect(displayValue('Cooking.Hob.Status.Zone.100.SelectedProgram', '0')).toBe('None');
    // A real programme number is a fact and stays one.
    expect(displayValue('BSH.Common.Root.ActiveProgram', '8200')).toBe('8200');
    // …and `0` is only special on a programme key. Elsewhere it is a number.
    expect(displayValue('Cooking.Hob.Setting.PowerLevel', '0')).toBe('0');
  });

  it('renders a count of seconds as hours and minutes, on the keys that carry seconds', () => {
    expect(displayValue('BSH.Common.Option.RemainingProgramTime', '3660')).toBe('1 h 1 min');
    expect(displayValue('BSH.Common.Option.ElapsedProgramTime', '600')).toBe('10 min');
    expect(displayValue('BSH.Common.Option.StartInRelative', '0')).toBe('0 min');
  });

  /**
   * Not every key ending in `Time`, `Duration` or `Relative` holds seconds: `ZoneSelectionTime` is the enum
   * `Infinite`/`Limited`, and `ApplianceDateTime` is a timestamp. A suffix rule alone would render
   * both as a duration.
   */
  it('leaves a time-shaped key alone when its value is not a count of seconds', () => {
    expect(displayValue('Cooking.Hob.Setting.ZoneSelectionTime', 'Infinite')).toBe('Infinite');
    expect(displayValue('BSH.Common.Setting.ApplianceDateTime', '2026-08-04T07:04:39+00:00')).toBe(
      '2026-08-04T07:04:39+00:00',
    );
  });

  it('labels a progress value with the unit it is measured in', () => {
    expect(displayValue('BSH.Common.Option.ProgramProgress', '0')).toBe('0%');
    expect(displayValue('BSH.Common.Option.ProgramProgress', '38')).toBe('38%');
    // Outside 0–100 it is not a percentage, and is not labelled as one.
    expect(displayValue('BSH.Common.Option.ProgramProgress', '4096')).toBe('4096');
  });

  it('summarises a list blob by the length the payload itself states', () => {
    expect(displayValue('BSH.Common.Status.ErrorCodesList', '{"length":0,"list":[]}')).toBe(
      'no items',
    );
    expect(
      displayValue('BSH.Common.Setting.Favorite.001.Program', '{"length":1,"list":[{"uid":1}]}'),
    ).toBe('1 item');
    expect(displayValue('BSH.Common.Root.StatusList', '{"length":7,"list":[]}')).toBe('7 items');
  });

  /**
   * A value this function does not recognise is returned untouched. Never "Unknown", never blank,
   * never guessed at.
   */
  it('passes anything it does not recognise straight through', () => {
    for (const value of [
      'Off',
      'Inactive',
      'IndicationOn',
      'Level0',
      'AllActive',
      '60seconds',
      'De',
      '-40',
      '3.5',
      '{"start":"2026-08-04T07:04:39+00:00"}',
    ]) {
      expect(displayValue('Some.Key.We.Have.Never.Seen', value)).toBe(value);
    }
    // Including when there is no key at all to reason from.
    expect(displayValue(null, 'Off')).toBe('Off');
  });
});

/**
 * The feature table renders what the daemon sent, so it gets only the absence correction and keeps
 * every real value verbatim, including the numbers and blobs that `displayValue` would summarise.
 */
describe('the_catalogue_only_corrects_the_word_null', () => {
  it('blanks an absent value and touches nothing else', () => {
    expect(blankValue(null)).toBe('—');
    expect(blankValue('null')).toBe('—');
    expect(blankValue('')).toBe('—');

    expect(blankValue('0')).toBe('0');
    expect(blankValue('true')).toBe('true');
    expect(blankValue('3660')).toBe('3660');
    expect(blankValue('{"length":0,"list":[]}')).toBe('{"length":0,"list":[]}');
  });
});

describe('a_programme_value_is_shown_by_name', () => {
  const programs = [
    { uid: 8192, feature_key: 'Dishcare.Dishwasher.Program.Eco50', label: 'Eco 50°' },
    { uid: 8193, feature_key: null, label: 'Quick' },
  ];

  it('names the programme by uid or by dotted key', () => {
    expect(programName('8192', programs)).toBe('Eco 50°');
    expect(programName('Dishcare.Dishwasher.Program.Eco50', programs)).toBe('Eco 50°');
    expect(displayValue('BSH.Common.Root.SelectedProgram', '8193', programs)).toBe('Quick');
  });

  it('shows an unknown programme as reported, and zero as none', () => {
    expect(programName('9999', programs)).toBeNull();
    expect(displayValue('BSH.Common.Root.ActiveProgram', '9999', programs)).toBe('9999');
    expect(displayValue('BSH.Common.Root.ActiveProgram', '0', programs)).toBe('None');
  });
});
