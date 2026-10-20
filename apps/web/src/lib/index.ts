/**
 * `$lib` barrel: the app name and the route table. The onboarding wizard, device dashboard and
 * frame inspector live in `$lib/components`; API shapes come from `$lib/api-types`.
 */

export const APP_NAME = 'Kessel';

/**
 * Client-side route IDs, one per page under `src/routes/`.
 *
 * These are route IDs, not hrefs: always pass one through `resolve()` from `$app/paths`. The app
 * is hash-routed to survive the Home Assistant ingress prefix (see `svelte.config.js`), so the
 * href for `/inspector` is `<runtime base>#/inspector`, and only `resolve()` knows the base.
 *
 * Under ingress, `base` is the document's directory without its trailing slash, so SvelteKit
 * classifies every in-app link as external and navigation becomes a full page load:
 *
 *   doc = http://127.0.0.1:8099/
 *     base = ''                              href = '#/inspector'          → client-side navigation
 *   doc = http://ha.local/api/hassio_ingress/<token>/
 *     base = '/api/hassio_ingress/<token>'   href = '…/<token>#/inspector' → full page load
 *
 * This is inherent to SvelteKit's base expression and is accepted. `resolve()` is still required
 * because it keeps the href inside the prefix; a bare `href="/inspector"` points at the host root
 * and leaves the add-on entirely.
 *
 * The full page load lands on the right route only because:
 *   1. the browser requests `…/api/hassio_ingress/<token>` (no trailing slash);
 *   2. the server 308-redirects to the same path with a trailing slash, built from
 *      `X-Ingress-Path` (see `svelte.config.js`);
 *   3. the browser re-applies the `#/inspector` fragment across the redirect;
 *   4. the shell boots at `…/<token>/` and the hash router reads the fragment.
 * If any step breaks, navigation silently lands on Home.
 */
export const ROUTES = {
  home: '/',
  inspector: '/inspector',
  config: '/config',
  /** Dynamic: pass the params to `resolve()` — `resolve(ROUTES.device, { slug })`. */
  device: '/devices/[slug]',
} as const;

export type RouteName = keyof typeof ROUTES;
