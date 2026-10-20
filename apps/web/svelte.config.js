import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/**
 * Kessel web UI: SvelteKit with `adapter-static`, built as a pure SPA.
 *
 * Output must go to `apps/web/build/`: `apps/server` embeds that directory with `rust-embed`, and
 * the `build` recipe builds the web app before the server. Changing `pages`/`assets` below breaks
 * the server's asset embedding.
 *
 * `fallback: 'index.html'` means the server needs one rule: serve the embedded file if it exists,
 * otherwise serve `index.html` with 200. Hash routing forces `ssr: false` on every page, so nothing
 * is server-rendered or prerendered and no Node runtime is needed in production.
 *
 * ── Serving behind the Home Assistant ingress prefix ────────────────────────────────────────────
 *
 * One build must work at `/` on loopback and under an ingress prefix (`/api/hassio_ingress/<token>/`)
 * that is unknown at build time. `paths.relative` alone does not achieve this: SvelteKit skips its
 * relative rewrite for the fallback page, which is the only page emitted here. Two things are needed:
 *
 *   1. `router.type: 'hash'`. This is the only mode that gives a fallback page a base computed at
 *      runtime (`new URL('.', location).pathname.slice(0, -1)`), so router links, the
 *      `_app/version.json` poll and the API/SSE URLs all pick up the prefix. Routes live in the
 *      fragment, so the server is only ever asked for the mount point.
 *
 *   2. `scripts/relativize-index.js`, run after `vite build`, rewrites the root-absolute `/_app/…`
 *      URLs in `build/index.html` to `./_app/…`. Hash routing leaves those rooted at `/`, where they
 *      404 behind the prefix and the page is blank.
 *
 * ── What the server must do under ingress ───────────────────────────────────────────────────────
 *
 * The Supervisor strips the prefix before proxying and passes it in the `X-Ingress-Path` header, so
 * the server sees a request path of `/` or `""`, never the prefixed one.
 *
 *   1. Serve the shell only for a path that ends in `/`. Both mechanisms above resolve against the
 *      document's directory; without the trailing slash every asset 404s one level too high.
 *
 *   2. On a bare or empty path, 308 to `{X-Ingress-Path}/`. Never `Location: /` (it escapes the
 *      prefix) and not a relative `./` either (against `…/<token>` it resolves to
 *      `/api/hassio_ingress/` and drops the token). On loopback the header is absent and the path
 *      already ends in `/`, so no redirect arises.
 *
 *      This redirect is on the hot path: under ingress every in-app link is a full page load (see
 *      `ROUTES` in `src/lib/index.ts`). The Location must carry no fragment, so the browser
 *      re-applies the original `#/route`.
 *
 *   3. Disable response buffering on the SSE route, or the frame inspector receives nothing until
 *      the connection closes.
 *
 * @type {import('@sveltejs/kit').Config}
 */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'index.html',
      precompress: false,
      strict: true,
    }),
    router: {
      // Required for the ingress prefix; see above.
      type: 'hash',
    },
    paths: {
      base: '',
      relative: true,
    },
  },
};

export default config;
