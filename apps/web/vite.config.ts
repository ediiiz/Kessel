import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  // Tailwind v4 is a Vite plugin, not a PostCSS step: there is no `tailwind.config.js` and no
  // `postcss.config.js`. The design tokens live in `src/app.css` under `@theme`. Tailwind must run
  // before SvelteKit so that `<style>` blocks and `app.css` are both processed.
  plugins: [tailwindcss(), sveltekit()],
  server: {
    // Dev server only; nothing here affects the static build the daemon embeds.
    port: 5173,
    strictPort: false,
    proxy: {
      // The daemon embeds the SPA at compile time, so without this every UI change needs a full
      // `just build`. With it, `vite dev` serves the UI with HMR and forwards the API to a daemon
      // started separately on its default port, 8099 (loopback only).
      //
      // The browser sees one origin, so the `SameSite=Strict` session cookie stays first-party and
      // `X-Kessel-Token` stays a same-origin header; neither CSRF property is weakened in dev.
      //
      // `/api/events` is SSE, a plain HTTP response rather than an upgrade, so `ws: false` is right;
      // it works because vite's proxy does not buffer. A WebSocket route would need `ws: true`.
      '/api': {
        target: 'http://127.0.0.1:8099',
        changeOrigin: false,
      },
    },
  },
});
