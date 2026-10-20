/**
 * Light / dark / follow-the-system, as one piece of module state.
 *
 * Tailwind's `dark:` variant is configured in `src/app.css` as `&:is(.dark *)`, a class rather than
 * a media query, so this module can override the OS preference. The class goes on `<html>`.
 *
 * `'system'` is the default and differs from picking light or dark because it keeps tracking: a user
 * who picks `'dark'` keeps a dark UI when their OS flips at dusk; a user on `'system'` does not.
 *
 * The initial class is applied by a blocking inline script in `src/app.html`, so the first paint is
 * already the right colour. That script and this module must agree on the storage key and values.
 */

export type ThemeChoice = 'light' | 'dark' | 'system';

/** Shared with the inline script in `src/app.html`; change both together. */
export const THEME_STORAGE_KEY = 'kessel.theme';

const CHOICES: readonly ThemeChoice[] = ['light', 'dark', 'system'];

function isChoice(value: string | null): value is ThemeChoice {
  return value !== null && (CHOICES as readonly string[]).includes(value);
}

/**
 * What was stored, or `'system'`.
 *
 * `localStorage` throws in a browser with site data blocked, which is realistic inside the Home
 * Assistant ingress iframe. A theme preference is not worth breaking the page over.
 */
function stored(): ThemeChoice {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    return isChoice(raw) ? raw : 'system';
  } catch {
    return 'system';
  }
}

function systemPrefersDark(): boolean {
  return globalThis.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false;
}

class Theme {
  /** The operator's choice. Assigning it writes the class and the store. */
  choice = $state<ThemeChoice>('system');
  /** Whether the system is currently dark. Tracked so `'system'` stays live. */
  systemDark = $state(false);

  /** What is actually on screen, after resolving `'system'`. */
  get dark(): boolean {
    return this.choice === 'system' ? this.systemDark : this.choice === 'dark';
  }
}

const theme = new Theme();

/**
 * Reads the stored choice and keeps `<html>`'s class in step with it. Call once, from the root
 * layout's `$effect`; the returned function detaches the media-query listener.
 */
export function startTheme(): () => void {
  theme.choice = stored();
  theme.systemDark = systemPrefersDark();

  const query = globalThis.matchMedia?.('(prefers-color-scheme: dark)');
  const onSystemChange = (event: MediaQueryListEvent): void => {
    theme.systemDark = event.matches;
  };
  query?.addEventListener('change', onSystemChange);
  return () => query?.removeEventListener('change', onSystemChange);
}

/** Applies `theme.dark` to `<html>`. Call from an `$effect` so it re-runs when the choice changes. */
export function applyTheme(): void {
  document.documentElement.classList.toggle('dark', theme.dark);
}

/** Records a choice. Persisting is best-effort for the same reason {@link stored} is. */
export function setTheme(choice: ThemeChoice): void {
  theme.choice = choice;
  try {
    localStorage.setItem(THEME_STORAGE_KEY, choice);
  } catch {
    // A preference that cannot be remembered still applies for this page.
  }
}

/** light → dark → system → light. */
export function cycleTheme(): void {
  setTheme(theme.choice === 'light' ? 'dark' : theme.choice === 'dark' ? 'system' : 'light');
}

export { theme };
