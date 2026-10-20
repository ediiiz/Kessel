/**
 * Component-test helpers, imported only from `*.test.ts` so they never reach the bundle.
 *
 * Uses Svelte 5's own `mount`/`unmount`/`flushSync` rather than a testing library, to avoid a
 * dependency for what amounts to "mount, poke, read back".
 */
import { flushSync, mount, unmount } from 'svelte';
import type { Component } from 'svelte';

export interface Mounted {
  /** The element the component was mounted into. */
  readonly container: HTMLElement;
  /** `[data-testid="…"]`, or `null`. Asserting on `null` is how "renders no control" is pinned. */
  readonly query: (testId: string) => HTMLElement | null;
  /**
   * `[data-testid="…"]` anywhere in the document, not only inside the container.
   *
   * Needed because a `bits-ui` dialog renders through a portal: its content is attached to
   * `document.body`, outside the mount container, so [`query`] cannot see it.
   *
   * Use [`query`] by default and this only for dialog content; reaching for it to find ordinary
   * markup hides a mount-target mistake.
   */
  readonly queryPortal: (testId: string) => HTMLElement | null;
  /** Every `[data-testid="…"]` match, in document order. */
  readonly queryAll: (testId: string) => HTMLElement[];
  /** Apply pending state changes synchronously. */
  readonly flush: () => void;
  readonly destroy: () => void;
}

/** Mounts `component` into a detached-but-attached container and returns query helpers. */
export function render<Props extends Record<string, unknown>>(
  component: Component<Props>,
  props: Props,
): Mounted {
  const container = document.createElement('div');
  document.body.append(container);
  const instance = mount(component, { target: container, props });
  flushSync();
  return {
    container,
    query: (testId) => container.querySelector<HTMLElement>(`[data-testid="${testId}"]`),
    queryPortal: (testId) => document.body.querySelector<HTMLElement>(`[data-testid="${testId}"]`),
    queryAll: (testId) => [...container.querySelectorAll<HTMLElement>(`[data-testid="${testId}"]`)],
    flush: () => {
      flushSync();
    },
    destroy: () => {
      void unmount(instance);
      container.remove();
    },
  };
}

/**
 * The text of every toast currently on screen, joined.
 *
 * Toasts are not addressable by `data-testid` — `svelte-sonner` owns the markup and renders it
 * outside whichever container raised it — so this reads sonner's own `data-sonner-toast` attribute.
 * Mount the app's `Toaster` (see `$lib/components/ui/sonner`) in the test first, or this is always
 * empty: nothing renders a toast that has no host.
 */
export function toastText(): string {
  return toastTexts().join('\n');
}

/**
 * The text of each toast currently on screen, one entry per toast.
 *
 * Use this over {@link toastText} when the *number* of toasts is the assertion — "sending…" being
 * replaced by "accepted" and "sending…" still sitting above it both satisfy a `toContain('accepted')`
 * on the joined string, and they are not the same behaviour.
 *
 * `[data-removed="true"]` is excluded. A dismissed toast stays in the DOM while sonner animates it
 * out, and in jsdom that animation never finishes, so counting the raw elements counts toasts that
 * a browser has already taken off the screen.
 */
export function toastTexts(): string[] {
  return [...document.querySelectorAll('[data-sonner-toast]:not([data-removed="true"])')].map(
    (toast) => toast.textContent ?? '',
  );
}

/** Clicks an element and flushes, failing loudly if it is not there. */
export function click(element: HTMLElement | null): void {
  if (element === null) {
    throw new Error('click() was given no element');
  }
  element.click();
  flushSync();
}

/** Types into an `<input>`: sets the value and fires the events Svelte binds to. */
export function type(element: HTMLElement | null, value: string): void {
  if (!(element instanceof HTMLInputElement)) {
    throw new Error('type() needs an <input>');
  }
  element.value = value;
  element.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

/** Chooses an option in a `<select>`: sets the value and fires the event Svelte listens for. */
export function select(element: HTMLElement | null, value: string): void {
  if (element instanceof HTMLSelectElement) {
    element.value = value;
    if (element.value !== value) {
      throw new Error(`select() was given "${value}", which is not one of the options`);
    }
    element.dispatchEvent(new Event('change', { bubbles: true }));
    flushSync();
    return;
  }

  // A shadcn/bits-ui `Select`: a trigger button over a listbox, not an `<option>` list. Opened with a
  // keypress rather than a click because bits-ui listens on pointer events, which a synthetic
  // `click()` does not carry; jsdom models the keyboard path faithfully.
  if (element === null) {
    throw new Error('select() was given null');
  }
  element.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  flushSync();

  const options = [...document.querySelectorAll<HTMLElement>('[role="option"]')];
  const wanted = options.find(
    (option) => option.dataset.value === value || option.textContent?.trim().startsWith(value),
  );
  if (wanted === undefined) {
    const offered = options.map((option) => option.textContent?.trim()).join(', ');
    throw new Error(`select() was given "${value}", which is not one of: ${offered}`);
  }
  wanted.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true }));
  wanted.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, cancelable: true }));
  wanted.click();
  flushSync();
}

/**
 * Attaches a file to a `<input type="file">`.
 *
 * `files` is read-only in jsdom and there is no `DataTransfer` constructor, so it is redefined.
 */
export function attachFile(element: HTMLElement | null, file: File): void {
  if (!(element instanceof HTMLInputElement)) {
    throw new Error('attachFile() needs an <input type="file">');
  }
  Object.defineProperty(element, 'files', { value: [file], configurable: true });
  element.dispatchEvent(new Event('change', { bubbles: true }));
  flushSync();
}

/** A minimal profile ZIP stand-in: the wizard only ever hands the bytes to the API. */
export function zipFile(name = 'profile.zip'): File {
  return new File([new Uint8Array([0x50, 0x4b, 0x03, 0x04])], name, { type: 'application/zip' });
}
