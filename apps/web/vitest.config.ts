import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

/**
 * Vitest configuration, kept separate from `vite.config.ts` so test settings cannot affect the
 * production build. Vitest prefers this file when both exist.
 *
 * `sveltekit()` is used rather than a bare `svelte()` plugin because it resolves the `$app/*`
 * virtual modules and the `$lib` alias, which lets a test mock `$app/paths` exactly as the real
 * build resolves it. `conditions: ['browser']` makes Svelte 5 resolve to its client runtime (its
 * `mount()` throws on the server build).
 */
export default defineConfig({
  plugins: [sveltekit()],
  resolve: {
    conditions: ['browser'],
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    include: ['src/**/*.test.ts'],
    restoreMocks: true,
  },
});
