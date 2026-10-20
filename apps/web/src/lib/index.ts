/**
 * `$lib` barrel: the app name and the route table. The onboarding wizard, device dashboard and
 * frame inspector live in `$lib/components`; API shapes come from `$lib/api-types`.
 */

export const APP_NAME = 'Kessel';

/**
 * Client-side route IDs, one per page under `src/routes/`.
 *
 * These are route IDs, not hrefs: always pass one through `link()` from `$lib/links`. The app is
 * hash-routed to survive the Home Assistant ingress prefix (see `svelte.config.js`), so the href
 * for `/inspector` is `<runtime base>/#/inspector`:
 *
 *   doc = http://127.0.0.1:8099/                       href = '#/inspector'
 *   doc = http://ha.local/api/hassio_ingress/<token>/  href = '/api/hassio_ingress/<token>/#/inspector'
 *
 * Both are the document's own path, so navigation stays client-side. A bare `href="/inspector"`
 * points at the host root and leaves the add-on entirely.
 */
export const ROUTES = {
  home: '/',
  inspector: '/inspector',
  config: '/config',
  /** Dynamic: pass the params to `resolve()` — `resolve(ROUTES.device, { slug })`. */
  device: '/devices/[slug]',
} as const;

export type RouteName = keyof typeof ROUTES;
