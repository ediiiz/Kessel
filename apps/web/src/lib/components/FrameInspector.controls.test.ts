/**
 * The inspector's *controls*, as distinct from what it renders: the preference that survives a page
 * load, the pause that freezes the view without closing the stream, and the tail-following the log
 * gives up when the operator scrolls away from it.
 *
 * Split from `FrameInspector.test.ts`: these cover what the operator can do to the log, rather than
 * what the daemon puts in it.
 */
import { describe, expect, it } from 'vitest';
import { click, render, select } from '$lib/dom-testing';
import {
  INSPECTOR_STORAGE_KEY,
  InspectorSettings,
  loadInspectorSettings,
} from '$lib/inspector-settings.svelte';
import FrameInspector from './FrameInspector.svelte';
import { fakeStream, frame, immediately } from './frame-inspector-fixtures';

/**
 * The filter costs a **reconnect** (the daemon applies it), and under Home Assistant ingress every
 * in-app navigation is a full page load — so a filter that is not remembered is a filter the operator
 * re-chooses on every visit. This pins the round trip through storage rather than the storage call:
 * what matters is that a *newly mounted* inspector subscribes with what the last one was told.
 */
describe('inspector_remembers_the_filter_between_visits', () => {
  it('persists the level and the sources, and opens the next stream with them', () => {
    localStorage.removeItem(INSPECTOR_STORAGE_KEY);
    const first = fakeStream();
    const view = render(FrameInspector, {
      subscribe: first.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    select(view.query('frame-level-filter'), 'warn');
    view.flush();
    click(view.query('frame-source-mqtt'));
    view.flush();
    view.destroy();

    expect(loadInspectorSettings()).toStrictEqual({
      level: 'warn',
      sources: ['mqtt'],
      follow: true,
    });

    // What the operator actually experiences: the page is loaded again, and the stream it opens is
    // the narrowed one, because the filter is in the URL.
    const second = fakeStream();
    const revisited = render(FrameInspector, {
      subscribe: second.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(loadInspectorSettings()),
    });
    expect(second.lastFilter()).toStrictEqual({ level: 'warn', sources: ['mqtt'] });
    revisited.destroy();
  });
});

/**
 * Pausing is a view action: the stream stays open and the ring keeps filling, so the events that
 * arrive while you are reading are not lost.
 */
describe('inspector_pause_freezes_the_view_without_losing_events', () => {
  it('holds the redraw, reports what is held, and shows it all on resume', () => {
    const stream = fakeStream();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings: new InspectorSettings(),
    });

    stream.deliver(frame(1), frame(2));
    view.flush();
    expect(view.queryAll('frame-row')).toHaveLength(2);

    click(view.query('frame-pause'));
    stream.deliver(frame(3), frame(4), frame(5));
    view.flush();

    // Frozen on screen, still arriving underneath, and saying so rather than looking disconnected.
    expect(view.queryAll('frame-row')).toHaveLength(2);
    expect(view.query('frame-count')?.textContent).toBe('2 events');
    expect(view.query('frame-held')?.textContent?.replaceAll(/\s+/gu, ' ').trim()).toBe(
      'paused — 3 held',
    );
    expect(stream.isConnected()).toBe(true);

    click(view.query('frame-pause'));
    view.flush();
    expect(view.queryAll('frame-row').map((row) => row.dataset.seq)).toStrictEqual([
      '1',
      '2',
      '3',
      '4',
      '5',
    ]);
    expect(view.query('frame-held')).toBeNull();
    view.destroy();
  });
});

/**
 * Tailing, and the reason it is not simply "scroll to the bottom on every batch".
 *
 * jsdom has no layout — every box is 0×0 and `scrollTop` never moves on its own — so the geometry is
 * supplied here. What is tested is the decision the component makes from a scroll position, not the
 * browser's scrolling.
 */
describe('inspector_stops_following_when_the_operator_scrolls_away', () => {
  it('parks the log on a scroll away, and re-engages on the jump control', () => {
    const stream = fakeStream();
    const settings = new InspectorSettings();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings,
    });

    stream.deliver(frame(1), frame(2));
    view.flush();
    // Following: nothing to jump to, because the view is already there.
    expect(view.query('frame-jump')).toBeNull();

    const box = view.query('frame-log');
    Object.defineProperty(box, 'scrollHeight', { value: 1000, configurable: true });
    Object.defineProperty(box, 'clientHeight', { value: 100, configurable: true });
    Object.defineProperty(box, 'scrollTop', { value: 300, configurable: true, writable: true });
    // Only a scroll that follows the operator's own input parks the log.
    box?.dispatchEvent(new Event('wheel'));
    box?.dispatchEvent(new Event('scroll'));
    view.flush();

    expect(settings.follow).toBe(false);
    expect(view.query('frame-parked')).not.toBeNull();
    expect(view.query('frame-jump')).not.toBeNull();

    // New rows keep arriving; the view stays where it was put.
    stream.deliver(frame(3));
    view.flush();
    expect(box?.scrollTop).toBe(300);

    click(view.query('frame-jump'));
    view.flush();
    expect(settings.follow).toBe(true);
    expect(box?.scrollTop).toBe(1000);
    expect(view.query('frame-jump')).toBeNull();
    view.destroy();
  });
});

/**
 * The arrival entrance.
 *
 * Rows carry `content-visibility: auto`, so a row the browser skipped does not run its animation
 * until it is painted — "animate every new row" therefore means old rows replaying their entrance
 * when the operator scrolls past them, minutes later. The class must land on the last few batches
 * only — long enough for the entrance to finish — and on nothing at all while the log is parked.
 */
describe('inspector_animates_only_the_batch_that_just_arrived', () => {
  it('marks the newest rows, retires the mark after a few batches, and skips a parked log', () => {
    const stream = fakeStream();
    const settings = new InspectorSettings();
    const view = render(FrameInspector, {
      subscribe: stream.subscribe,
      schedule: immediately,
      settings,
    });
    const marked = (): (string | undefined)[] =>
      view
        .queryAll('frame-row')
        .filter((row) => row.classList.contains('frame-row-in'))
        .map((row) => row.dataset.seq);

    // Note what a "batch" is here: the injected scheduler publishes synchronously, so each
    // delivered frame is its own batch, where a browser would coalesce 100 ms of them into one.
    //
    // The first publish is the daemon's retained ring arriving in one go. A thousand rows of
    // backlog is not an arrival, and none of it animates.
    stream.deliver(frame(1));
    view.flush();
    expect(marked()).toStrictEqual([]);

    stream.deliver(frame(2));
    view.flush();
    expect(marked()).toStrictEqual(['2']);

    // A row keeps its mark for three batches (~300 ms, enough for the entrance), then loses it —
    // which is what stops an unpainted row from animating whenever it is eventually scrolled to.
    stream.deliver(frame(3));
    view.flush();
    expect(marked()).toStrictEqual(['2', '3']);
    stream.deliver(frame(4));
    view.flush();
    stream.deliver(frame(5));
    view.flush();
    expect(marked()).toStrictEqual(['3', '4', '5']);

    // Parked: the rows are arriving somewhere the operator is not looking.
    settings.follow = false;
    view.flush();
    stream.deliver(frame(6));
    view.flush();
    expect(marked()).toStrictEqual([]);
    view.destroy();
  });
});
