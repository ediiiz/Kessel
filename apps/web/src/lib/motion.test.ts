/**
 * The motion system's arithmetic.
 *
 * jsdom has no compositor, so only the timing is testable here, not the movement. What matters is
 * that a stagger stays bounded (dozens of quick actions must not take seconds to arrive) and that
 * reduced motion collapses every helper to nothing rather than merely shortening it.
 */
import { describe, expect, it } from 'vitest';
import { enter, enterTiming, flash, MOTION } from './motion';

/** `enter`/`flash` are Svelte transitions and take a node they never read in these cases. */
const node = (): Element => document.createElement('div');

describe('motion_is_budgeted_and_gives_way_to_the_operator', () => {
  it('delays each item by its index, so a list arrives as a sequence rather than a slab', () => {
    expect(enterTiming(false, { index: 0 }).delay).toBe(0);
    expect(enterTiming(false, { index: 1 }).delay).toBe(MOTION.step);
    expect(enterTiming(false, { index: 3 }).delay).toBe(3 * MOTION.step);
  });

  it('caps the stagger, so the oven’s 35th quick action does not start 1.6 s late', () => {
    const capped = MOTION.steps * MOTION.step;

    expect(enterTiming(false, { index: MOTION.steps }).delay).toBe(capped);
    expect(enterTiming(false, { index: 34 }).delay).toBe(capped);
    // Also a wall-clock ceiling: the last thing on screen must still feel like part of the arrival.
    expect(capped).toBeLessThanOrEqual(400);
  });

  it('collapses to nothing under reduced motion — no delay, no duration, no movement', () => {
    const timing = enterTiming(true, { index: 6, y: 24, duration: 900 });

    expect(timing).toEqual({ delay: 0, duration: 0, y: 0 });
  });

  it('rises into place: transparent and offset at the start, at rest and opaque at the end', () => {
    const config = enter(node(), { index: 0, y: 12 });

    const start = config.css?.(0, 1) ?? '';
    const end = config.css?.(1, 0) ?? '';

    expect(start).toContain('opacity: 0');
    expect(start).toContain('12px');
    expect(end).toContain('opacity: 1');
    expect(end).toContain('0px');
  });

  it('flashes a changed value in place, and not at all when motion is reduced', () => {
    const config = flash(node());

    // A flash fades and nothing else; a value that also moves is harder to read as it changes.
    expect(config.duration).toBe(MOTION.slow);
    expect(config.css?.(0, 1) ?? '').not.toContain('translate');

    expect(flash(node(), { reduced: true }).duration).toBe(0);
  });
});
