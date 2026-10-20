/**
 * What a unit test can pin and a component test cannot: what happens when the list moves rather
 * than the operator.
 *
 * jsdom has no layout, so the geometry is supplied here. The subject is the decision `Tail` makes
 * from a scroll position, not the browser's scrolling.
 */
import { describe, expect, it } from 'vitest';
import { InspectorSettings } from './inspector-settings.svelte';
import { Tail } from './inspector-tail';

/** A scroll box with the geometry a browser would have computed, and a `scrollTop` that moves. */
function box(geometry: { scrollHeight: number; clientHeight: number }): HTMLElement {
  const element = document.createElement('div');
  Object.defineProperty(element, 'scrollHeight', {
    value: geometry.scrollHeight,
    configurable: true,
  });
  Object.defineProperty(element, 'clientHeight', { value: geometry.clientHeight });
  // jsdom's own `scrollTop` is pinned at 0 because nothing has a layout; this one remembers what was
  // written to it, which is the whole of what `Tail` needs from the element.
  Object.defineProperty(element, 'scrollTop', { value: 0, writable: true });
  return element;
}

describe('inspector_tail_keeps_following_when_the_list_grows_under_it', () => {
  it('does not read its own pin, or a late measurement, as the operator scrolling away', () => {
    const settings = new InspectorSettings();
    const tail = new Tail(() => settings);
    tail.box = box({ scrollHeight: 1000, clientHeight: 100 });

    tail.pin();
    expect(tail.box.scrollTop).toBe(1000);

    // What `content-visibility: auto` does: rows revealed by that pin are measured at paint, so the
    // box is suddenly 400px taller and the view 400px from the bottom with no operator action.
    Object.defineProperty(tail.box, 'scrollHeight', { value: 1400, configurable: true });
    tail.onScroll();

    expect(settings.follow).toBe(true);
  });

  it('does not read a smooth scroll still on its way down as the operator leaving', () => {
    const settings = new InspectorSettings();
    const tail = new Tail(() => settings);
    tail.box = box({ scrollHeight: 1000, clientHeight: 100 });
    tail.pin();

    // Mid-animation: the box is between where it was and the bottom.
    tail.box.scrollTop = 600;
    tail.onScroll();

    expect(settings.follow).toBe(true);
  });

  it('parks the log when the operator moves the box somewhere else', () => {
    const settings = new InspectorSettings();
    const tail = new Tail(() => settings);
    tail.box = box({ scrollHeight: 1000, clientHeight: 100 });
    tail.pin();

    tail.onIntent();
    tail.box.scrollTop = 200;
    tail.onScroll();
    expect(settings.follow).toBe(false);

    // And scrolling back down re-engages it: the gesture everyone tries before finding the button.
    tail.box.scrollTop = 900;
    tail.onScroll();
    expect(settings.follow).toBe(true);
  });

  it('glides to a nearby bottom but jumps across the scrollback', () => {
    const settings = new InspectorSettings();
    const tail = new Tail(() => settings);
    tail.box = box({ scrollHeight: 1000, clientHeight: 100 });

    tail.pin();
    expect(tail.box.style.scrollBehavior).toBe('auto');

    tail.box.scrollTop = 850;
    tail.pin();
    expect(tail.box.style.scrollBehavior).toBe('');
  });

  it('finishes a glide that came to rest short of the bottom, unless parked', () => {
    const settings = new InspectorSettings();
    const tail = new Tail(() => settings);
    tail.box = box({ scrollHeight: 1000, clientHeight: 100 });
    const detach = tail.watch(tail.box);

    tail.box.scrollTop = 700;
    tail.box.dispatchEvent(new Event('scrollend'));
    expect(tail.box.scrollTop).toBe(1000);

    settings.follow = false;
    tail.box.scrollTop = 700;
    tail.box.dispatchEvent(new Event('scrollend'));
    expect(tail.box.scrollTop).toBe(700);
    detach();
  });

  it('does nothing at all before the element exists', () => {
    const settings = new InspectorSettings();
    const tail = new Tail(() => settings);

    tail.pin();
    tail.onScroll();

    expect(settings.follow).toBe(true);
  });
});
