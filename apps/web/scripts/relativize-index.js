/**
 * Post-build step: make `build/index.html`'s asset URLs document-relative.
 *
 * Hash routing gives the client a runtime base, but SvelteKit still emits the fallback page's
 * `<link rel=modulepreload>`, `<link rel=stylesheet>` and bootstrap `import()` URLs rooted at `/`.
 * Behind the Home Assistant ingress prefix those 404 and the page is blank. This rewrites
 * `"/_app/…"` to `"./_app/…"`, which resolves against the document's directory, the same base
 * SvelteKit computes for the client router. Vite already emits chunk-to-chunk imports as relative.
 *
 * It fails the build if the file is missing, if nothing was rewritten, or if a root-absolute
 * reference survives, so a SvelteKit upgrade that changes the output cannot silently ship a bundle
 * that does not boot behind ingress. If it reports "rewrote nothing", SvelteKit may now emit
 * relative URLs itself and this script can be deleted.
 *
 * Relative resolution requires the shell to be served at a path ending in `/`; see
 * `../svelte.config.js` for what the server must do under ingress.
 *
 * Limitation: only `build/index.html` is scanned, and only quoted references match, so an unquoted
 * CSS `url(/_app/…)` would go unnoticed. The stylesheet has no `url(` references today; re-check
 * when the first font or image is imported from CSS.
 *
 * Not type-checked: `scripts/` is outside the generated tsconfig's `include`, so the JSDoc here is
 * documentation only. `oxlint` and `oxfmt` do run on it.
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const INDEX = resolve(import.meta.dirname, '..', 'build', 'index.html');

/** Matches a root-absolute reference to the immutable asset dir inside an HTML/JS string. */
const ROOT_ABSOLUTE_APP = /(["'`])\/_app\//gu;

/** @param {string} message */
function fail(message) {
  process.stderr.write(`relativize-index: ${message}\n`);
  process.exit(1);
}

let html;
try {
  html = readFileSync(INDEX, 'utf8');
} catch {
  fail(`cannot read ${INDEX} — did \`vite build\` run?`);
  throw new Error('unreachable');
}

const rewritten = html.replace(ROOT_ABSOLUTE_APP, '$1./_app/');
const count = (html.match(ROOT_ABSOLUTE_APP) ?? []).length;

if (count === 0) {
  fail(
    'found no root-absolute "/_app/" references to rewrite. Either SvelteKit now emits relative ' +
      'asset URLs for a hash-routed fallback page (in which case delete this script and the ' +
      'note in svelte.config.js), or the build output moved. Do not silence this.',
  );
}

// Unreachable while the replacement above is global; a tripwire for a future edit that narrows it.
if (/(["'`])\/_app\//u.test(rewritten)) {
  fail('rewrite left root-absolute "/_app/" references behind — refusing to ship.');
}

writeFileSync(INDEX, rewritten);
process.stdout.write(
  `relativize-index: rewrote ${String(count)} root-absolute /_app/ references in build/index.html\n`,
);
