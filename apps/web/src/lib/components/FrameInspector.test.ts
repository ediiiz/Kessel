import { describe, expect, it } from 'vitest';
import { click, render, select } from '$lib/dom-testing';
import { InspectorSettings } from '$lib/inspector-settings.svelte';
import FrameInspector from './FrameInspector.svelte';
import { fakeStream, frame, immediately } from './frame-inspector-fixtures';

/*
 * Every case mounts with its own `InspectorSettings`, never the persisted singleton.
 *
 * The settings are remembered across page loads, so a shared instance would carry one case's filter
 * into the next. The one case that is about persistence uses `localStorage` itself.
 */

describe('inspector_renders_sse_frames_live', () => {
  it('renders frames as they arrive, in order, with their kind and level', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    expect(view.queryAll('frame-row')).toHaveLength(0);

    stream.deliver(
      frame(1, { kind: 'connection', level: 'info', message: 'connecting' }),
      frame(2, { kind: 'frame', level: 'debug', message: 'GET /ci/services' }),
      frame(3, { kind: 'log', level: 'warn', message: 'retrying' }),
    );
    view.flush();

    const rows = view.queryAll('frame-row');
    expect(rows).toHaveLength(3);
    expect(rows.map((row) => row.dataset.seq)).toStrictEqual(['1', '2', '3']);
    expect(view.queryAll('frame-kind').map((cell) => cell.textContent?.trim())).toStrictEqual([
      'connection',
      'frame',
      'log',
    ]);
    expect(view.queryAll('frame-level').map((cell) => cell.textContent?.trim())).toStrictEqual([
      'info',
      'debug',
      'warn',
    ]);
    expect(rows[1]?.textContent).toContain('GET /ci/services');
    // The decoded body must be shown; `fields` is where the daemon puts it.
    expect(rows[1]?.textContent).toContain('direction');
    view.destroy();
  });
});

/**
 * What a reconnect looks like *to this component*: `EventSource` re-establishes the connection
 * itself and the daemon may re-send frames the browser already has (it sends `id: <seq>`, so a
 * resume is possible; whether it replays is the daemon's business). The subscription is therefore
 * continuous and the component must survive both a **replay** and a **gap**.
 */
describe('inspector_survives_stream_reconnect', () => {
  it('does not duplicate replayed frames, and does not silently drop missing ones', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    stream.deliver(frame(1), frame(2), frame(3));
    view.flush();
    expect(view.queryAll('frame-row').map((row) => row.dataset.seq)).toStrictEqual(['1', '2', '3']);

    // The connection drops and resumes; seq 2 and 3 arrive a second time, then a new one.
    stream.deliver(frame(2), frame(3), frame(4));
    view.flush();
    expect(view.queryAll('frame-row').map((row) => row.dataset.seq)).toStrictEqual([
      '1',
      '2',
      '3',
      '4',
    ]);

    // `seq` is gap-free, so a gap means frames were dropped, and that must be visible: seq 5..8
    // never arrived.
    stream.deliver(frame(9));
    view.flush();
    expect(view.queryAll('frame-row').map((row) => row.dataset.seq)).toStrictEqual([
      '1',
      '2',
      '3',
      '4',
      '9',
    ]);
    const gaps = view.queryAll('frame-gap');
    expect(gaps).toHaveLength(1);
    // Exact, not `toContain('4')`: a substring match is satisfied by the seq numbers already in
    // the sentence, so a gap claiming the wrong count would still pass.
    expect(gaps[0]?.textContent?.replaceAll(/\s+/gu, ' ').trim()).toBe(
      '4 events dropped between seq 4 and 9',
    );
    expect(view.query('frame-count')?.textContent).toBe('5 events');
    expect(stream.isConnected()).toBe(true);
    view.destroy();
  });
});

describe('inspector_clear_resets_the_log_and_keeps_the_stream', () => {
  it('discards accumulated rows on an explicit click, and keeps receiving', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    // Nothing to clear yet, so the control is inoperable rather than absent-and-surprising.
    expect(view.query('frame-clear')?.hasAttribute('disabled')).toBe(true);

    stream.deliver(frame(1), frame(2), frame(5));
    view.flush();
    expect(view.queryAll('frame-row')).toHaveLength(3);
    expect(view.queryAll('frame-gap')).toHaveLength(1);
    expect(view.query('frame-clear')?.hasAttribute('disabled')).toBe(false);

    click(view.query('frame-clear'));
    view.flush();

    expect(view.queryAll('frame-row')).toHaveLength(0);
    expect(view.queryAll('frame-gap')).toHaveLength(0);
    expect(view.query('frame-count')?.textContent).toBe('0 events');
    expect(view.query('no-frames')).not.toBeNull();

    // The subscription is untouched: clearing is a view action, not a disconnect.
    expect(stream.isConnected()).toBe(true);
    stream.deliver(frame(6));
    view.flush();
    expect(view.queryAll('frame-row').map((row) => row.dataset.seq)).toStrictEqual(['6']);
    // And the cleared log starts a fresh sequence baseline: no gap is invented for seq 1..5.
    expect(view.queryAll('frame-gap')).toHaveLength(0);
    view.destroy();
  });
});

/**
 * The filter is the daemon's, not the browser's: narrowing it reopens the stream with a new query
 * string, so the excluded events are never sent at all. A component that kept the old subscription
 * and hid rows would pass any on-screen assertion while leaving the full stream on the wire.
 */
describe('inspector_filters_are_applied_by_the_daemon', () => {
  it('re-subscribes with the chosen level and sources, and starts a clean log', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    expect(stream.lastFilter()).toStrictEqual({ level: 'trace', sources: [] });

    stream.deliver(frame(1), frame(2));
    view.flush();
    expect(view.queryAll('frame-row')).toHaveLength(2);

    select(view.query('frame-level-filter'), 'warn');
    view.flush();

    expect(stream.lastFilter()).toStrictEqual({ level: 'warn', sources: [] });
    // The rows on screen were collected under the old filter; keeping them would mix two streams.
    expect(view.queryAll('frame-row')).toHaveLength(0);
    expect(view.query('frame-count')?.textContent).toBe('0 events');
    expect(stream.isConnected()).toBe(true);
    view.destroy();
  });
});

// Its own `describe` so the block above stays within `max-lines-per-function`.
describe('inspector_source_chips_select_subsystems', () => {
  it('accumulates the source chips, and reopens the stream on each one', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    click(view.query('frame-source-mqtt'));
    view.flush();
    expect(stream.lastFilter()).toStrictEqual({ level: 'trace', sources: ['mqtt'] });

    // A second chip adds to the selection rather than replacing it.
    click(view.query('frame-source-http'));
    view.flush();
    expect(stream.lastFilter()?.sources).toStrictEqual(['mqtt', 'http']);

    // And toggling one back off removes it.
    click(view.query('frame-source-mqtt'));
    view.flush();
    expect(stream.lastFilter()?.sources).toStrictEqual(['http']);
    expect(stream.isConnected()).toBe(true);
    view.destroy();
  });
});

describe('inspector_does_not_call_a_filtered_hole_a_drop', () => {
  it('stops reporting dropped events while a filter is on, and says so', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    // Unfiltered, a hole in `seq` is a drop and is reported.
    stream.deliver(frame(1), frame(5));
    view.flush();
    expect(view.queryAll('frame-gap')).toHaveLength(1);
    expect(view.query('frame-gaps-off')).toBeNull();

    select(view.query('frame-level-filter'), 'error');
    view.flush();

    // Filtered, the holes are the events the operator excluded — the daemon does not renumber —
    // so reporting them as dropped would be wrong.
    stream.deliver(frame(11, { level: 'error' }), frame(30, { level: 'error' }));
    view.flush();
    expect(view.queryAll('frame-row')).toHaveLength(2);
    expect(view.queryAll('frame-gap')).toHaveLength(0);
    // But the operator is told what they gave up, rather than left to assume nothing was lost.
    expect(view.query('frame-gaps-off')).not.toBeNull();
    view.destroy();
  });
});
