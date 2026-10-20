import { describe, expect, it } from 'vitest';
import { featureContext, featureSection, namesakeHints, spellMember } from './features';

describe('featureSection', () => {
  it('groups by the kind segment wherever it sits in the key', () => {
    expect(featureSection('Dishcare.Dishwasher.Setting.TimeLight')).toBe('Settings');
    expect(featureSection('BSH.Common.Setting.Favorite.001.Functionality')).toBe('Settings');
    expect(featureSection('Dishcare.Dishwasher.Option.IntensivZone')).toBe('Options');
    expect(featureSection('BSH.Common.Status.DoorState')).toBe('Status');
    expect(featureSection('Dishcare.Dishwasher.Program.Eco50')).toBe('Programmes');
    expect(featureSection('BSH.Common.Command.AbortProgram')).toBe('Commands');
    expect(featureSection('BSH.Common.Event.ProgramFinished')).toBe('Events');
    expect(featureSection('BSH.Common.Root.ActiveProgram')).toBe('Programmes');
  });

  it('puts a key without a kind, or no key, under Other', () => {
    expect(featureSection('Some.Feature')).toBe('Other');
    expect(featureSection(null)).toBe('Other');
  });
});

describe('namesakeHints', () => {
  it('separates namesakes by the segments between kind and name', () => {
    expect(featureContext('Dishcare.Dishwasher.Setting.LearningDishwasher.Feedback.Cleaning')).toBe(
      'Learning dishwasher · Feedback',
    );
    const hints = namesakeHints([
      {
        uid: 1,
        label: 'Cleaning',
        feature_key: 'D.D.Setting.LearningDishwasher.Feedback.Cleaning',
      },
      {
        uid: 2,
        label: 'Cleaning',
        feature_key: 'D.D.Setting.LearningDishwasher.InitialPreference.Cleaning',
      },
      { uid: 3, label: 'Power', feature_key: 'BSH.Common.Setting.PowerState' },
    ]);
    expect(hints.get(1)).toBe('Learning dishwasher · Feedback');
    expect(hints.get(2)).toBe('Learning dishwasher · Initial preference');
    expect(hints.has(3)).toBe(false);
  });

  it('falls back to the last segment when the context is the same, and skips one equal to the name', () => {
    const hints = namesakeHints([
      { uid: 1, label: 'Interior light', feature_key: 'D.D.Setting.InteriorLight' },
      { uid: 2, label: 'Interior light', feature_key: 'D.D.Setting.InteriorLightMode' },
    ]);
    expect(hints.has(1)).toBe(false);
    expect(hints.get(2)).toBe('Interior light mode');
  });
});

describe('spellMember', () => {
  it('breaks camel case into a sentence', () => {
    expect(spellMember('ColdWater')).toBe('Cold water');
    expect(spellMember('24Hours')).toBe('24 hours');
  });

  it('spells programme the way the catalogue does', () => {
    expect(spellMember('LastProgram')).toBe('Last programme');
    expect(spellMember('Program')).toBe('Programme');
    expect(spellMember('Programmable')).toBe('Programmable');
  });
});
