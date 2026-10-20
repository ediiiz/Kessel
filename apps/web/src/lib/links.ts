import { resolve } from '$app/paths';

/**
 * The href for an in-app route: `resolve()` with a `/` kept between the base and the fragment.
 *
 * Under hash routing SvelteKit's base is the document's directory without its trailing slash, so
 * `resolve()` gives `/api/hassio_ingress/<token>#/inspector`. That path is not the document's
 * (`…/<token>/`), so the browser leaves the page for it, and Home Assistant answers 404: its
 * ingress route only matches `…/<token>/…`. Keeping the slash makes every link same-document, so
 * the hash router navigates without a reload, at `/` on loopback and under any ingress prefix.
 */
export const link: typeof resolve = (...args) =>
  // The spread loses the pairing of route and params that `typeof resolve` already checked.
  withinDocument((resolve as (...forwarded: typeof args) => string)(...args)) as ReturnType<
    typeof resolve
  >;

/** Inserts the `/` before the fragment when the path part does not already end in one. */
export function withinDocument(href: string): string {
  return href.replace(/^([^#]*[^#/])#/u, '$1/#');
}
