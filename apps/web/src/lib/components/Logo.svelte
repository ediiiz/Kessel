<script lang="ts">
  /**
   * The Kessel mark: a **K** whose arm is answered by a detached arc.
   *
   * The stem and chevron are the monogram; the arc suggests a live connection to an appliance that
   * talks back. It is an arc rather than a second chevron, which would read as a "back" button.
   *
   * The paths are inline rather than an `<img>` or CSS `background-image`: an asset URL emitted by
   * Vite may not be document-relative behind the Home Assistant ingress prefix (see
   * `scripts/relativize-index.js`), and inline SVG has no URL at all. It also inherits
   * `currentColor`, so the mark follows the theme.
   *
   * Other copies of the mark: `static/favicon.svg` carries the same three paths, and the add-on store
   * needs PNGs, `addon/icon.png` (128 px, the mark on its tile) and `addon/logo.png` (460 × 150,
   * mark plus wordmark in Instrument Sans). The PNGs were drawn from these paths on a `<canvas>` and
   * saved with `toDataURL`. Change the geometry here and update all three.
   *
   * `bg-primary` / `text-primary-foreground` keep the tile's contrast in both themes. `aria-hidden`
   * because the wordmark beside it already says "Kessel".
   */
  import { cn } from '$lib/utils.js';

  /**
   * `class` sizes the tile from the call site, because the two shells differ: the sidebar header has
   * room for `size-7`, the phone top bar does not. The glyph is sized as a *percentage of the tile*
   * rather than by a second prop, so a caller cannot pick a tile and a glyph that disagree.
   */
  const { class: className }: { class?: string } = $props();
</script>

<span
  class={cn(
    'bg-primary text-primary-foreground flex size-7 shrink-0 items-center justify-center rounded-md',
    className,
  )}
  aria-hidden="true"
>
  <svg viewBox="0 0 32 32" class="h-auto w-[72%]" fill="none">
    <rect x="5.4" y="6" width="3.6" height="20" rx="1.8" fill="currentColor" />
    <path
      d="M18.2 7.6 9.4 16l8.8 8.4"
      stroke="currentColor"
      stroke-width="3.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
    <path
      d="M23.4 10.4a8.6 8.6 0 0 1 0 11.2"
      stroke="currentColor"
      stroke-width="2.8"
      stroke-linecap="round"
      opacity="0.5"
    />
  </svg>
</span>
