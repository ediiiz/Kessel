<script lang="ts">
  import { link } from '$lib/links';
  import { Activity, Plus, RefreshCw, Settings, TriangleAlert } from '$lib/icons';
  import type { DeviceSummary } from '$lib/api-types';
  import { createApiClient } from '$lib/api';
  import { debounce, RESYNC_DEBOUNCE_MS, subscribeLive } from '$lib/live-shell';
  import { Skeleton } from '$lib/components/ui/skeleton';
  import AddApplianceSheet from '$lib/components/AddApplianceSheet.svelte';
  import DeviceList from '$lib/components/DeviceList.svelte';
  import { APP_NAME, ROUTES } from '$lib/index.js';
  import { enter, leave } from '$lib/motion';
  import { refresh, session } from '$lib/session.svelte';
  import { getToken } from '$lib/token';
  import { cn } from '$lib/utils.js';

  // Both credentials are offered: a token still in `sessionStorage` (an operator mid-sign-in, or one who
  // prefers not to open a session) and the session cookie. See `makeAuthHeaders` for the precedence.
  const api = createApiClient({ token: () => getToken(), hasSession: () => session.authenticated });
  let devices = $state<DeviceSummary[]>([]);
  let failure = $state<string | null>(null);
  let loading = $state(true);
  const sheet = $state({ open: false });

  /** The hue of the glow behind the page, by time of day. */
  function glowHue(hour: number): number {
    if (hour >= 5 && hour < 12) return 60;
    if (hour >= 12 && hour < 18) return 220;
    return 290;
  }

  const hue = glowHue(new Date().getHours());

  const SHORTCUT = 'press flex flex-col items-center gap-2 text-center text-sm font-medium';

  const blank = $derived(loading && devices.length === 0);

  async function load(): Promise<void> {
    loading = true;
    failure = null;
    try {
      devices = await api.devices();
    } catch (error) {
      failure = error instanceof Error ? error.message : 'the daemon could not be reached';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void load();
    void refresh(api);
  });

  $effect(() => {
    const resync = debounce(() => void load(), RESYNC_DEBOUNCE_MS);
    const unsubscribe = subscribeLive({
      onResync: () => {
        resync.call();
      },
    });
    return () => {
      resync.cancel();
      unsubscribe();
    };
  });
</script>

{#snippet bubble(Icon: typeof Plus, hue: number)}
  <span
    class="scene flex size-[4.5rem] items-center justify-center rounded-full text-white shadow-(--shadow-soft)"
    style:--hue={hue}
  >
    <Icon class="relative z-10 size-7" aria-hidden="true" />
  </span>
{/snippet}

<svelte:head>
  <title>{APP_NAME}</title>
</svelte:head>

<AddApplianceSheet
  open={sheet.open}
  onopenchange={(open) => (sheet.open = open)}
  {api}
  onadded={() => void load()}
/>

<div
  class="pointer-events-none absolute inset-x-0 top-0 h-[34rem] overflow-hidden [mask-image:linear-gradient(to_bottom,black_45%,transparent)]"
  aria-hidden="true"
  style:--hue={hue}
>
  <div
    class="absolute -top-40 -right-24 size-[30rem] rounded-full bg-[oklch(0.82_0.14_var(--hue))] opacity-70 blur-3xl dark:bg-[oklch(0.45_0.15_var(--hue))] dark:opacity-40"
  ></div>
  <div
    class="absolute -top-24 -left-32 size-[22rem] rounded-full bg-[oklch(0.86_0.1_calc(var(--hue)+40))] opacity-50 blur-3xl dark:bg-[oklch(0.4_0.13_calc(var(--hue)+40))]"
  ></div>
</div>

<div class="relative flex flex-col gap-8">
  <section class="flex flex-col gap-4 pt-6" in:enter|global={{ index: 0 }}>
    <div class="flex items-center gap-2">
      <h1 class="mr-auto text-[2.2rem] leading-tight font-semibold tracking-tight">Appliances</h1>
      <button
        type="button"
        onclick={load}
        disabled={loading}
        title="Refresh"
        aria-label="Refresh"
        class="press glass text-muted-foreground hover:text-foreground flex size-10 shrink-0 items-center justify-center rounded-full border disabled:opacity-60"
      >
        <RefreshCw class={cn('size-4', loading && 'animate-spin')} aria-hidden="true" />
      </button>
      <button
        type="button"
        onclick={() => (sheet.open = true)}
        data-testid="add-appliance"
        title="Add appliance"
        aria-label="Add appliance"
        class="press border-primary/40 text-primary hover:bg-primary/10 flex size-10 items-center justify-center rounded-full border-2"
      >
        <Plus class="size-5" aria-hidden="true" />
      </button>
    </div>

    {#if failure !== null}
      <div
        class="border-destructive/40 bg-destructive/5 text-destructive flex items-start gap-2 rounded-2xl border p-4 text-sm"
        data-testid="dashboard-error"
      >
        <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <span>{failure}</span>
      </div>
    {:else}
      <div class="stack">
        {#if blank}
          <ul
            class="flex gap-3 overflow-hidden sm:grid sm:grid-cols-2 lg:grid-cols-3"
            aria-busy="true"
            out:leave
          >
            {#each [0, 1, 2] as placeholder (placeholder)}
              <li class="glass w-[72%] shrink-0 overflow-hidden rounded-3xl border sm:w-auto">
                <div class="flex flex-col gap-2 p-4">
                  <Skeleton class="h-5 w-28" />
                  <Skeleton class="h-4 w-20" />
                </div>
                <Skeleton class="h-36 rounded-none" />
              </li>
            {/each}
          </ul>
        {:else}
          <div in:enter>
            <DeviceList {devices} onadd={() => (sheet.open = true)} />
          </div>
        {/if}
      </div>
    {/if}
  </section>

  <section class="flex flex-col gap-4" in:enter|global={{ index: 1 }}>
    <h2 class="text-2xl font-semibold tracking-tight">Shortcuts</h2>
    <div class="grid grid-cols-3 gap-3">
      <button type="button" onclick={() => (sheet.open = true)} class={SHORTCUT}>
        {@render bubble(Plus, 255)}
        Add appliance
      </button>
      <a href={link(ROUTES.inspector)} class={SHORTCUT}>
        {@render bubble(Activity, 175)}
        Frames
      </a>
      <a href={link(ROUTES.config)} class={SHORTCUT}>
        {@render bubble(Settings, 300)}
        Settings
      </a>
    </div>
  </section>
</div>
