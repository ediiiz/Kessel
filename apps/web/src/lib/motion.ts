/**
 * The motion system: one entrance, one flash, one reorder, and a budget. Durations live in one
 * place so that things on a page do not arrive at different speeds.
 *
 *   - {@link enter}: a section rises and fades in, staggered by its position, in the order the page
 *     wants read.
 *   - {@link flash}: a value that just changed washes clay and fades back. This is information, not
 *     decoration: it tells the operator which number on a mostly static page just moved.
 *   - {@link flipMs}: the duration for `animate:flip` on a short list that reorders.
 *
 * ── Budget ──
 *
 * A transition is a compositor layer plus a per-frame style write, so it goes on a section, a card
 * or a row of a short list. Never on a row of the feature catalogue (hundreds of rows) or of the
 * frame log (hundreds of events per second).
 *
 * The frame log's one exception is `app.css`'s `frame-row-in`, a one-shot opacity keyframe on the
 * newest batch only, which costs no main-thread work per frame. Two lessons from it for any surface
 * that mixes motion with `content-visibility: auto`: a skipped row runs its animation only when it is
 * first painted, so animating every new row replays entrances on scroll; and a `transform` entrance
 * inside a self-scrolling container reads as jitter. Opacity only, newest batch only.
 *
 * The stagger is capped at {@link MOTION.steps}, so a screen with dozens of quick actions still
 * settles within about 700 ms.
 *
 * ── `|global` is required ──
 *
 * Every use of {@link enter} is written `in:enter|global={{ … }}`. Svelte transitions are local by
 * default and only play when their own block changes, not when a parent block creates them, and
 * nearly every entrance here is created by a parent (`{:else}` branches, the route `{#key}`). Without
 * `|global` nothing animates, and nothing warns.
 *
 * {@link flash} is deliberately local: it lives inside a `{#key value}` block, so a value change
 * fires it while the initial page build does not.
 *
 * ── Reduced motion ──
 *
 * `prefers-reduced-motion: reduce` collapses every helper to zero duration and zero displacement,
 * not a faster version of the same movement. `app.css` carries a global backstop for CSS-only
 * movement that never reaches this module. The `reduced` option on each function exists so the
 * arithmetic is testable without a media query; components do not pass it.
 */
import { cubicOut } from 'svelte/easing';
import { prefersReducedMotion } from 'svelte/motion';
import type { TransitionConfig } from 'svelte/transition';

/**
 * Every duration and offset in the app, in milliseconds and pixels.
 *
 * `base` is the entrance. `fast` is for things that must not be waited on, such as a hover or press.
 * `slow` is the flash, long enough that the tint is still there when the eye arrives.
 */
export const MOTION = {
  /** Hover, press, and anything on the pointer's critical path. */
  fast: 140,
  /** The entrance. */
  base: 260,
  /** The value flash — long enough to be noticed after the fact. */
  slow: 520,
  /** One stagger step. */
  step: 45,
  /** How many steps the stagger is allowed before it stops growing. */
  steps: 8,
} as const;

export interface EnterOptions {
  /** Position in the sequence. Multiplied by {@link MOTION.step}, capped at {@link MOTION.steps}. */
  readonly index?: number;
  /** How far it rises, in pixels. Small: this is a settle, not an arrival from off-screen. */
  readonly y?: number;
  readonly duration?: number;
  /** Test seam. Defaults to the media query. */
  readonly reduced?: boolean;
}

export interface Timing {
  readonly delay: number;
  readonly duration: number;
  readonly y: number;
}

/** Whether the operator has asked for less movement. */
export function reducedMotion(): boolean {
  return prefersReducedMotion.current;
}

/**
 * What an entrance is allowed to cost: the stagger cap and the reduced-motion collapse. Pure, so a
 * test can check it without a compositor.
 */
export function enterTiming(reduced: boolean, options: EnterOptions = {}): Timing {
  if (reduced) {
    return { delay: 0, duration: 0, y: 0 };
  }
  const index = Math.max(0, options.index ?? 0);
  return {
    delay: Math.min(index, MOTION.steps) * MOTION.step,
    duration: options.duration ?? MOTION.base,
    y: options.y ?? 10,
  };
}

/**
 * A section rising into place: `in:enter={{ index }}`.
 *
 * Only opacity and `translate3d`, so the movement stays on the compositor and never reflows
 * siblings. Used as `in:`, not `transition:`: an outro would keep the old page on screen while the
 * new one arrives.
 */
export function enter(node: Element, options: EnterOptions = {}): TransitionConfig {
  void node;
  const { delay, duration, y } = enterTiming(options.reduced ?? reducedMotion(), options);
  return {
    delay,
    duration,
    easing: cubicOut,
    css: (t, u) => `opacity: ${String(t)}; transform: translate3d(0, ${String(u * y)}px, 0);`,
  };
}

/**
 * A placeholder fading out under what replaced it: `out:leave` on a skeleton inside a `stack`, so
 * the content's own entrance plays over it rather than after a blank frame.
 */
export function leave(
  node: Element,
  options: { readonly reduced?: boolean } = {},
): TransitionConfig {
  void node;
  return {
    duration: (options.reduced ?? reducedMotion()) ? 0 : MOTION.base,
    easing: cubicOut,
    css: (t) => `opacity: ${String(t)};`,
  };
}

/**
 * A value that just changed, washing clay and fading back: `{#key value}<span in:flash>…</span>{/key}`.
 *
 * Only a background tint: a value that moves as it changes is harder to read at the moment it
 * matters.
 *
 * The `{#key}` wrapper makes this fire only on a change: the block is recreated when the value
 * changes, and an `in:` transition does not play on the initial render, so a freshly loaded page
 * does not flash every tile.
 */
export function flash(
  node: Element,
  options: { readonly reduced?: boolean } = {},
): TransitionConfig {
  void node;
  return {
    duration: (options.reduced ?? reducedMotion()) ? 0 : MOTION.slow,
    easing: cubicOut,
    // `u` runs 1 → 0, so the tint is strongest when the value appears. Mixed from the accent token so
    // it follows the theme.
    css: (_t, u) =>
      `background-color: color-mix(in oklab, var(--primary) ${String(u * 20)}%, transparent);`,
  };
}

/**
 * The duration for `animate:flip` on a list that reorders under a filter.
 *
 * Only for short lists such as `DeviceList` and `QuickActions`, never the feature catalogue (see the
 * budget note above).
 */
export function flipMs(reduced: boolean = reducedMotion()): number {
  return reduced ? 0 : MOTION.base;
}
