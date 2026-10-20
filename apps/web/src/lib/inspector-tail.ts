/**
 * Whether the frame inspector's log is following its own tail, and the scrolling that decides it.
 *
 * Auto-scroll is only useful while the operator is reading the newest row; scroll up to inspect a
 * handshake and the view must stay put. The switch lives in `$lib/inspector-settings.svelte.ts` and
 * is persisted, so a parked log stays parked across the full page reloads Home Assistant ingress
 * causes.
 *
 * The hard part is telling our scrolling from the operator's. The box scrolls smoothly, and rows
 * use `content-visibility: auto`, so the view is often short of the bottom with no operator action.
 * So only a scroll that follows the operator's own input (wheel, touch, pointer, keys — see
 * [`onIntent`]) can park the log. Scrolling back to the bottom re-engages following.
 *
 * Nothing here is reactive except `settings.follow`, which this writes and the component reads.
 */
import type { InspectorSettings } from '$lib/inspector-settings.svelte';

/** How close to the bottom still counts as "watching the tail", in pixels. */
const TAIL_SLACK = 24;

/** The input that means the operator, not the tail, is moving the box. */
const INTENT_EVENTS = ['wheel', 'touchstart', 'pointerdown', 'keydown'] as const;

/** Further from the bottom than this many box heights, a pin jumps instead of gliding. */
const GLIDE_SCREENS = 2;

/** How long after the operator's input a scroll is still taken as theirs, in milliseconds. */
const INTENT_WINDOW = 800;

/**
 * How many times a pin may chase a bottom that is still moving before it gives up.
 *
 * Ordinary tailing settles on the first step; only a jump across the whole scrollback needs more.
 * The cap makes a list that never stops growing degrade to "slightly behind the bottom" rather
 * than a re-pin loop that never yields.
 */
const SETTLE_STEPS = 4;

export class Tail {
  /** The scroll container. `bind:this` writes it; a class field is the writable target that needs. */
  box: HTMLElement | null = null;
  /** The list height [`pin`] last scrolled to, or `-1` before it has ever run. */
  #pinnedHeight = -1;
  /** When the operator last touched, wheeled, clicked or keyed the box. */
  #intentAt = Number.NEGATIVE_INFINITY;
  /** Which convergence chain is the live one. See {@link follow}. */
  #settling = 0;
  /**
   * The settings object, read through a getter rather than captured.
   *
   * `FrameInspector.svelte` passes `() => settings`, where `settings` is a prop. Capturing the value
   * at construction would leave this writing `follow` on an object the component no longer renders.
   */
  readonly #settings: () => InspectorSettings;

  constructor(settings: () => InspectorSettings) {
    this.#settings = settings;
  }

  /**
   * Puts the view on the newest row and remembers the height.
   *
   * Near the bottom it glides, through the box's CSS `scroll-behavior`, so arriving rows ease in.
   * From further away it jumps: a smooth scroll across the scrollback is slow, and it passes rows
   * that are measured only as it reaches them, so it would land short.
   */
  pin(): void {
    const box = this.box;
    if (box !== null) {
      const fromBottom = box.scrollHeight - box.scrollTop - box.clientHeight;
      box.style.scrollBehavior = fromBottom > box.clientHeight * GLIDE_SCREENS ? 'auto' : '';
      box.scrollTop = box.scrollHeight;
      this.#pinnedHeight = box.scrollHeight;
    }
  }

  /**
   * Pins now, then keeps pinning until the bottom stops moving.
   *
   * A row skipped by `content-visibility: auto` keeps its placeholder height until it is painted, so
   * a pin lands on placeholder arithmetic and the bottom moves again once the revealed rows are
   * measured. The correction converges over several steps, each two animation frames apart because
   * the paint after the write is what does the measuring.
   *
   * {@link SETTLE_STEPS} bounds it, and it stops as soon as a pin does not move, which is the common
   * case. `#settling` is a token rather than a flag so that overlapping redraws do not start
   * overlapping chains; only the newest describes the current bottom.
   *
   * Guarded because jsdom, where the component tests run, has no animation frames.
   */
  follow(): void {
    this.pin();
    this.#settle((this.#settling += 1), SETTLE_STEPS);
  }

  /** One convergence step: re-pin after the paint that measured the last one. */
  #settle(token: number, remaining: number): void {
    if (remaining <= 0 || typeof requestAnimationFrame !== 'function') {
      return;
    }
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (token !== this.#settling || !this.#settings().follow) {
          return;
        }
        const before = this.#pinnedHeight;
        this.pin();
        if (this.#pinnedHeight !== before) {
          this.#settle(token, remaining - 1);
        }
      });
    });
  }

  /** The operator touched, wheeled, clicked or keyed the box: the next scroll is theirs. */
  onIntent(): void {
    this.#intentAt = performance.now();
  }

  /**
   * A scroll came to rest or the list resized. If the log is following but short of the bottom —
   * rows measured during a glide moved it — finish the job.
   */
  catchUp(): void {
    const box = this.box;
    if (box === null || !this.#settings().follow) {
      return;
    }
    if (box.scrollHeight - box.scrollTop - box.clientHeight > TAIL_SLACK) {
      this.pin();
    }
  }

  /**
   * Re-pins whenever the list's height changes while following.
   *
   * The settle loop in {@link follow} covers the frames right after a write; rows revealed later
   * (by a glide, or by a font or a wrap) move the bottom without any write. Returns the detach;
   * a no-op where `ResizeObserver` does not exist (jsdom).
   */
  track(list: HTMLElement): () => void {
    if (typeof ResizeObserver !== 'function') {
      return () => {};
    }
    const observer = new ResizeObserver(() => {
      this.catchUp();
    });
    observer.observe(list);
    return () => {
      observer.disconnect();
    };
  }

  /** Listens on `box` for the operator's input and for scrolls coming to rest; returns the detach. */
  watch(box: HTMLElement): () => void {
    const intent = (): void => {
      this.onIntent();
    };
    const rest = (): void => {
      this.catchUp();
    };
    for (const kind of INTENT_EVENTS) {
      box.addEventListener(kind, intent, { passive: true });
    }
    box.addEventListener('scrollend', rest);
    return () => {
      for (const kind of INTENT_EVENTS) {
        box.removeEventListener(kind, intent);
      }
      box.removeEventListener('scrollend', rest);
    };
  }

  /** A scroll happened: decides whether it was the operator leaving the tail. */
  onScroll(): void {
    const box = this.box;
    if (box === null) {
      return;
    }
    const fromBottom = box.scrollHeight - box.scrollTop - box.clientHeight;
    if (fromBottom <= TAIL_SLACK) {
      this.#settings().follow = true;
    } else if (performance.now() - this.#intentAt < INTENT_WINDOW) {
      this.#settings().follow = false;
    }
  }

  /** The jump control: re-engage following and go there now, rather than at the next batch. */
  jump(): void {
    this.#settings().follow = true;
    this.follow();
  }
}
