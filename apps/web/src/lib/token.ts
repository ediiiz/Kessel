/**
 * Where the operator's API token lives in the browser.
 *
 * The token gates every mutating request and is an operator credential:
 *
 *   - `sessionStorage`, not `localStorage`: it dies with the tab, but survives the full page loads
 *     that every in-app navigation becomes under the Home Assistant ingress prefix. An in-memory
 *     variable would be erased by those navigations and force the operator to re-paste it.
 *   - Never logged: this module does no logging, and `no-console` is an error repo-wide.
 *   - Never in a URL: `$lib/api` sends it only in the `X-Kessel-Token` header.
 *
 * Anything running in this origin can read `sessionStorage`. The daemon binds loopback and the SPA
 * loads no third-party code, so the origin's scripts are the binary's own embedded assets. An
 * `HttpOnly` cookie would be stricter, but the daemon does not issue one.
 */

/** The `sessionStorage` key. */
export const TOKEN_STORAGE_KEY = 'kessel.api-token';

/** The token, or `null` if none is held. Whitespace is not a token. */
export function getToken(): string | null {
  const stored = readSession();
  if (stored === null || stored.trim() === '') {
    return null;
  }
  return stored;
}

export function hasToken(): boolean {
  return getToken() !== null;
}

/** Stores the token for this tab. A blank value clears it rather than storing emptiness. */
export function setToken(token: string): void {
  if (token.trim() === '') {
    clearToken();
    return;
  }
  try {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, token);
  } catch {
    // A browser with storage denied still gets a working read-only UI; writes will report the
    // missing token through `MissingTokenError` rather than failing silently.
  }
}

export function clearToken(): void {
  try {
    sessionStorage.removeItem(TOKEN_STORAGE_KEY);
  } catch {
    // Nothing to do: there is no token to protect if storage is unavailable.
  }
}

function readSession(): string | null {
  try {
    return sessionStorage.getItem(TOKEN_STORAGE_KEY);
  } catch {
    return null;
  }
}
