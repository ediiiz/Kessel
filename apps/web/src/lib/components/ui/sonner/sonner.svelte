<script lang="ts">
  /**
   * The app's one toast host, mounted by the shell.
   *
   * This is shadcn-svelte's `sonner` item with two deliberate edits, both of which exist to avoid a
   * second source of truth:
   *
   *  - **Theme.** The registry version reads `mode-watcher`. This app already owns light/dark/system
   *    in `$lib/theme.svelte`, including the class on `<html>` and the no-flash inline script in
   *    `app.html`, so the toaster reads *that* instead. Two theme stores would disagree the moment an
   *    operator cycles the control in the sidebar. `mode-watcher` was removed from `package.json`
   *    again after the CLI added it.
   *  - **Icons.** Imported from the `$lib/icons` barrel for the reason that file documents — oxlint's
   *    `import/max-dependencies` counts each glyph as a dependency.
   *
   * The `--normal-*` variables are how sonner is told to use the app's palette; they are read by
   * `svelte-sonner`'s own stylesheet, so they must stay inline here rather than moving to `app.css`.
   */
  import { Toaster as Sonner, type ToasterProps as SonnerProps } from 'svelte-sonner';
  import { CircleCheck, Info, LoaderCircle, OctagonX, TriangleAlert } from '$lib/icons';
  import { theme } from '$lib/theme.svelte';

  const { ...restProps }: SonnerProps = $props();
</script>

<Sonner
  theme={theme.dark ? 'dark' : 'light'}
  class="toaster group"
  style="--normal-bg: var(--color-popover); --normal-text: var(--color-popover-foreground); --normal-border: var(--color-border);"
  {...restProps}
>
  {#snippet loadingIcon()}
    <LoaderCircle class="size-4 animate-spin" />
  {/snippet}
  {#snippet successIcon()}
    <CircleCheck class="size-4" />
  {/snippet}
  {#snippet errorIcon()}
    <OctagonX class="size-4" />
  {/snippet}
  {#snippet infoIcon()}
    <Info class="size-4" />
  {/snippet}
  {#snippet warningIcon()}
    <TriangleAlert class="size-4" />
  {/snippet}
</Sonner>
