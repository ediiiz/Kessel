/**
 * What the inspector's preference has to survive.
 *
 * Most cases here are about not trusting storage: the value is JSON that may have been written by
 * an earlier version of the app or anything else on the origin, and it is fed straight into the
 * `EventSource` query the daemon parses. So "restore what was there" is only half the contract;
 * the other half is "restore nothing the daemon has not declared".
 */
import { beforeEach, describe, expect, it } from 'vitest';
import {
  INSPECTOR_DEFAULTS,
  INSPECTOR_STORAGE_KEY,
  InspectorSettings,
  loadInspectorSettings,
  parseInspectorSettings,
  saveInspectorSettings,
} from './inspector-settings.svelte';

beforeEach(() => {
  localStorage.removeItem(INSPECTOR_STORAGE_KEY);
});

describe('inspector_settings_survive_a_page_load', () => {
  it('round-trips a choice through storage', () => {
    saveInspectorSettings({ level: 'warn', sources: ['mqtt', 'http'], follow: false });

    expect(loadInspectorSettings()).toStrictEqual({
      level: 'warn',
      sources: ['mqtt', 'http'],
      follow: false,
    });
  });

  it('hands back the firehose when nothing has been stored', () => {
    expect(loadInspectorSettings()).toStrictEqual(INSPECTOR_DEFAULTS);
  });

  it('starts a settings object from what was stored, filter included', () => {
    saveInspectorSettings({ level: 'error', sources: ['daemon'], follow: false });

    const settings = new InspectorSettings(loadInspectorSettings());

    expect(settings.filter).toStrictEqual({ level: 'error', sources: ['daemon'] });
    expect(settings.follow).toBe(false);
    // The snapshot is plain data, not the reactive proxy — that is what gets serialised.
    expect(settings.snapshot()).toStrictEqual({
      level: 'error',
      sources: ['daemon'],
      follow: false,
    });
  });
});

describe('inspector_settings_do_not_trust_what_they_read', () => {
  it('drops a level and a source group the daemon never declared', () => {
    const parsed = parseInspectorSettings({
      level: 'chatty',
      sources: ['mqtt', 'bluetooth', 7],
      follow: 'yes',
    });

    // The level and `follow` fall back rather than being sent as a query the daemon cannot parse;
    // the one recognised group survives.
    expect(parsed).toStrictEqual({ level: 'trace', sources: ['mqtt'], follow: true });
  });

  it('normalises the source order, so two spellings of one stream are one stream', () => {
    // `isFiltered` treats "every group" as no restriction; the order comes from `TRACE_SOURCES`, so
    // a selection made in a different order cannot produce a different query string.
    expect(parseInspectorSettings({ sources: ['http', 'mqtt', 'mqtt'] }).sources).toStrictEqual([
      'mqtt',
      'http',
    ]);
  });

  it('falls back rather than throwing on text that is not JSON', () => {
    localStorage.setItem(INSPECTOR_STORAGE_KEY, '{not json');

    expect(loadInspectorSettings()).toStrictEqual(INSPECTOR_DEFAULTS);
  });
});
