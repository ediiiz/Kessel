<script lang="ts">
  /**
   * The device page's full-bleed hero: the role's artwork with the name and connection state over
   * it, a back button and a refresh. The model line shows `brand` and `vib`, which are not private.
   */
  import { resolve } from '$app/paths';
  import type { DeviceSummary } from '$lib/api-types';
  import {
    connectionDetail,
    connectionLook,
    modelLine,
    roleLabel,
    type Tone,
  } from '$lib/appliance';
  import ApplianceArt from '$lib/components/ApplianceArt.svelte';
  import { ArrowLeft, RefreshCw } from '$lib/icons';
  import { ROUTES } from '$lib/index.js';
  import { cn } from '$lib/utils.js';

  const {
    device,
    slug,
    loading,
    onreload,
  }: {
    device: DeviceSummary | null;
    slug: string;
    loading: boolean;
    onreload: () => void;
  } = $props();

  const look = $derived(connectionLook(device?.connection_state ?? 'offline'));
  const detail = $derived(
    connectionDetail(device?.connection_state ?? 'offline', device?.last_failure ?? null),
  );

  const DOT: Record<Tone, string> = {
    live: 'bg-success shadow-[0_0_0_3px_color-mix(in_oklch,var(--success)_30%,transparent)]',
    working: 'bg-warning animate-pulse',
    idle: 'bg-white/50',
    bad: 'bg-destructive animate-pulse',
  };

  const BUTTON =
    'press flex size-11 items-center justify-center rounded-full border border-white/25 bg-white/15 text-white backdrop-blur-xl hover:bg-white/25 disabled:opacity-60';
</script>

<header class="relative -mx-4 overflow-hidden rounded-b-[2rem] md:mx-0 md:rounded-[2rem]">
  <ApplianceArt role={device?.role ?? ''} size="hero" class="h-[19rem] md:h-80" />
  <div
    class="absolute inset-0 z-20 bg-gradient-to-t from-black/55 via-black/10 to-transparent"
    aria-hidden="true"
  ></div>

  <div
    class="absolute inset-x-0 top-0 z-30 flex items-center justify-between px-4 pt-[calc(1rem+env(safe-area-inset-top))] md:px-6"
  >
    <a href={resolve(ROUTES.home)} class={BUTTON} aria-label="Back to home" title="Back">
      <ArrowLeft class="size-5" aria-hidden="true" />
    </a>
    <button
      type="button"
      onclick={onreload}
      disabled={loading}
      title="Refresh"
      aria-label="Refresh this appliance"
      class={BUTTON}
    >
      <RefreshCw class={cn('size-4', loading && 'animate-spin')} aria-hidden="true" />
    </button>
  </div>

  <div class="absolute inset-x-0 bottom-0 z-30 flex flex-col gap-2 px-5 pb-10 text-white md:px-7">
    <h1 class="text-4xl font-semibold tracking-tight drop-shadow-sm">
      {roleLabel(device?.role ?? slug)}
    </h1>
    {#if device !== null}
      <div class="flex flex-wrap items-center gap-2 text-sm">
        <span
          class="inline-flex items-center gap-2 rounded-full border border-white/25 bg-white/15 px-3 py-1 font-medium backdrop-blur-xl"
        >
          <span class={cn('size-2 rounded-full', DOT[look.tone])} aria-hidden="true"></span>
          <span class="capitalize">{device.connection_state}</span>
        </span>
        {#if modelLine(device) !== null}
          <span class="font-mono text-xs text-white/75" data-testid="device-model"
            >{modelLine(device)}</span
          >
        {/if}
      </div>
      {#if detail !== null}
        <p class="text-sm text-white/80" data-testid="device-connection-detail">{detail}</p>
      {/if}
    {/if}
  </div>
</header>
