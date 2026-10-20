import { afterAll } from 'vitest';

/**
 * `Element.prototype.animate`, which jsdom does not implement at all.
 *
 * Svelte 5 drives every `transition:`/`in:`/`out:` through the Web Animations API — even a
 * zero-delay intro creates a dummy `element.animate(…, { duration: 0, fill: 'forwards' })` so the
 * keyframes are not built until the DOM has updated. Without this, mounting *any* component
 * carrying a transition throws `element.animate is not a function`, which took out all eight
 * `WriteControl` cases the moment its root gained one.
 *
 * The stub is inert in the same way the `matchMedia` one below is. jsdom has no compositor and
 * applies no animated styles, so there is nothing here for a test to observe and no test asserts on
 * one; what it must do is complete, so the transition machinery runs its `onfinish` and tears down
 * instead of leaving the element pinned in its pre-intro state. `onfinish` is therefore fired on a
 * microtask, and `cancel()` suppresses it — which is exactly how Svelte aborts a transition that is
 * interrupted, and getting that wrong would leak a callback into the next test.
 *
 * **This does not test motion, and it is not meant to.** `src/lib/motion.test.ts` covers the timing
 * arithmetic; the movement itself is verified in a browser and nowhere else.
 */
function stubAnimate(): Animation {
  let cancelled = false;
  const animation = {
    onfinish: null as (() => void) | null,
    oncancel: null as (() => void) | null,
    effect: null,
    // Svelte's tick loop bails unless this reads something other than `'running'`, which is what
    // keeps a jsdom test from spinning a `requestAnimationFrame` loop per transition.
    playState: 'finished',
    currentTime: 0,
    startTime: 0,
    cancel(): void {
      cancelled = true;
    },
    play(): void {},
    pause(): void {},
    finish(): void {},
  };
  queueMicrotask(() => {
    if (!cancelled) {
      animation.onfinish?.();
    }
  });
  return animation as unknown as Animation;
}

if (globalThis.Element !== undefined && Element.prototype.animate === undefined) {
  Element.prototype.animate = stubAnimate as typeof Element.prototype.animate;
}

if (typeof window !== 'undefined' && window.matchMedia === undefined) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string): MediaQueryList =>
      ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => false,
      }) as unknown as MediaQueryList,
  });
}

// bits-ui restores the body style 24 ms after a dialog closes. Let that timer run before the file's
// environment is torn down, or it fires against a missing `document`.
afterAll(async () => {
  await new Promise((resolve) => {
    setTimeout(resolve, 50);
  });
});
