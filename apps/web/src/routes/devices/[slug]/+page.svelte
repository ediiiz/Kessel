<script lang="ts" module>
  import type { DeviceSummary, EntityDescriptor, ProgramDescriptor } from '$lib/api-types';

  /** The last load of each appliance, so returning to one paints at once and refreshes after. */
  const visited = new Map<
    string,
    { device: DeviceSummary | null; entities: EntityDescriptor[]; programs: ProgramDescriptor[] }
  >();

  /** A load faster than this shows no skeleton: a flash of one reads as flicker. */
  const SKELETON_DELAY_MS = 250;
</script>

<script lang="ts">
  /**
   * One appliance: the hero, what needs attention, the programme, quick actions, status, and every
   * feature in a sheet. Write controls exist only for an appliance that declares a writable feature;
   * on a heating appliance every write is confirmed first.
   */
  import { page } from '$app/state';
  import * as Alert from '$lib/components/ui/alert';
  import { Skeleton } from '$lib/components/ui/skeleton';
  import { ChevronRight, DoorOpen, Flame, SlidersHorizontal, TriangleAlert } from '$lib/icons';
  import type { WriteOutcome, WriteRequest } from '$lib/api-types';
  import { createApiClient } from '$lib/api';
  import { debounce, RESYNC_DEBOUNCE_MS, subscribeLive } from '$lib/live-shell';
  import DeviceHeader from '$lib/components/DeviceHeader.svelte';
  import EntityOverview from '$lib/components/EntityOverview.svelte';
  import FeatureSheet from '$lib/components/FeatureSheet.svelte';
  import ProgramList from '$lib/components/ProgramList.svelte';
  import ProgrammeCard from '$lib/components/ProgrammeCard.svelte';
  import QuickActions from '$lib/components/QuickActions.svelte';
  import SessionControl from '$lib/components/SessionControl.svelte';
  import { APP_NAME } from '$lib/index.js';
  import { isHeating, roleHue } from '$lib/appliance';
  import { enter, leave } from '$lib/motion';
  import { session, refresh } from '$lib/session.svelte';
  import { getToken } from '$lib/token';

  const api = createApiClient({ token: () => getToken(), hasSession: () => session.authenticated });
  const slug = $derived(page.params.slug ?? '');

  let device = $state<DeviceSummary | null>(null);
  let entities = $state<EntityDescriptor[]>([]);
  let programs = $state<ProgramDescriptor[]>([]);
  let failure = $state<string | null>(null);
  let loading = $state(true);
  let slow = $state(false);

  /** Waiting for an appliance with nothing of it to show yet. */
  const waiting = $derived(loading && device?.slug !== slug);
  const blank = $derived(waiting && slow);
  const sheet = $state({ open: false });
  const changeable = $derived(entities.filter((entity) => entity.writable).length);

  /** Shown on the programme card instead of the status tiles. */
  const PROGRAMME_KEYS = [
    'Root.ActiveProgram',
    'Root.SelectedProgram',
    'Option.ProgramProgress',
    'Option.RemainingProgramTime',
  ] as const;

  const doorOpen = $derived(
    entities.some(
      (entity) =>
        entity.feature_key?.endsWith('Status.DoorState') === true &&
        entity.value?.toLowerCase().endsWith('open') === true,
    ),
  );

  async function load(forSlug: string): Promise<void> {
    const seen = visited.get(forSlug);
    if (seen !== undefined && device?.slug !== forSlug) {
      ({ device, entities, programs } = seen);
    }
    loading = true;
    failure = null;
    const skeleton = setTimeout(() => {
      slow = true;
    }, SKELETON_DELAY_MS);
    try {
      const [devices, features, runnable] = await Promise.all([
        api.devices(),
        api.entities(forSlug),
        api.programs(forSlug),
      ]);
      device = devices.find((candidate) => candidate.slug === forSlug) ?? null;
      entities = features;
      programs = runnable;
      visited.set(forSlug, { device, entities, programs });
    } catch (error) {
      failure = error instanceof Error ? error.message : 'the device could not be loaded';
    } finally {
      clearTimeout(skeleton);
      slow = false;
      loading = false;
    }
  }

  const write = (target: string, request: WriteRequest): Promise<WriteOutcome> =>
    api.write(target, request);

  $effect(() => {
    void load(slug);
    void refresh(api);
  });

  function applyValue(uid: number, value: string): void {
    const at = entities.findIndex((entity) => entity.uid === uid);
    const current = entities[at];
    if (current === undefined) {
      return;
    }
    const next = [...entities];
    next[at] = { ...current, value };
    entities = next;
  }

  $effect(() => {
    const resync = debounce(() => void load(slug), RESYNC_DEBOUNCE_MS);
    const unsubscribe = subscribeLive({
      onValue: (update) => {
        if (update.slug !== slug) {
          return;
        }
        applyValue(update.uid, update.value);
      },
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

<svelte:head>
  <title>{slug} — {APP_NAME}</title>
</svelte:head>

<div class="stack">
  {#if failure !== null}
    <div class="pt-5">
      <Alert.Root variant="destructive" data-testid="device-error">
        <TriangleAlert aria-hidden="true" />
        <Alert.Title>This appliance could not be loaded</Alert.Title>
        <Alert.Description>{failure}</Alert.Description>
      </Alert.Root>
    </div>
  {:else if waiting && !blank}
    <div class="min-h-[100dvh]" aria-busy="true"></div>
  {:else if blank}
    <div class="flex flex-col gap-4" data-testid="device-loading" aria-busy="true" out:leave>
      <Skeleton class="-mx-4 h-[19rem] rounded-none md:mx-0 md:rounded-[2rem]" />
      <Skeleton class="h-36 rounded-3xl" />
      <div class="grid grid-cols-2 gap-3">
        {#each [0, 1, 2, 3] as placeholder (placeholder)}
          <Skeleton class="h-28 rounded-3xl" />
        {/each}
      </div>
    </div>
  {:else}
    <div class="flex flex-col gap-7">
      <div in:enter|global={{ index: 0 }}>
        <DeviceHeader {device} {slug} {loading} onreload={() => void load(slug)} />
      </div>

      <div
        class="pointer-events-none absolute inset-x-0 top-64 h-[60rem] overflow-hidden [mask-image:linear-gradient(to_bottom,black,transparent)]"
        aria-hidden="true"
        style:--hue={roleHue(device?.role ?? '')}
      >
        <div
          class="absolute top-0 -left-32 size-[28rem] rounded-full bg-[oklch(0.85_0.1_var(--hue))] opacity-60 blur-3xl dark:bg-[oklch(0.42_0.14_var(--hue))] dark:opacity-50"
        ></div>
        <div
          class="absolute top-80 -right-32 size-[24rem] rounded-full bg-[oklch(0.88_0.08_calc(var(--hue)+50))] opacity-50 blur-3xl dark:bg-[oklch(0.4_0.12_calc(var(--hue)+50))]"
        ></div>
      </div>

      <div class="relative -mt-5 flex flex-col gap-7">
        {#if doorOpen || (device !== null && isHeating(device.role))}
          <div class="relative z-30 flex flex-col gap-2" in:enter|global={{ index: 1 }}>
            {#if doorOpen}
              <div
                class="glass flex items-center gap-3 rounded-2xl border px-4 py-3 text-sm font-semibold"
                data-testid="device-door-open"
              >
                <span
                  class="bg-warning/20 text-warning flex size-8 items-center justify-center rounded-full"
                >
                  <DoorOpen class="size-4" aria-hidden="true" />
                </span>
                Door open
              </div>
            {/if}
            {#if device !== null && isHeating(device.role)}
              <div
                class="glass flex items-center gap-3 rounded-2xl border px-4 py-3 text-sm"
                data-testid="device-heat-warning"
              >
                <span
                  class="bg-destructive/15 text-destructive flex size-8 shrink-0 items-center justify-center rounded-full"
                >
                  <Flame class="size-4" aria-hidden="true" />
                </span>
                <span class="font-semibold">Changes are confirmed first</span>
              </div>
            {/if}
          </div>
        {/if}

        <div class="relative z-30 flex flex-col gap-7">
          <ProgrammeCard {entities} {programs} index={2} />

          {#if device !== null && device.ever_writable}
            <SessionControl {api} />
            <QuickActions {entities} {slug} role={device.role} {write} index={3} />
          {/if}

          <EntityOverview {entities} {programs} exclude={PROGRAMME_KEYS} index={4} />

          <ProgramList {programs} index={5} />

          <div in:enter|global={{ index: 6 }}>
            <button
              type="button"
              class="glass press flex w-full items-center gap-4 rounded-3xl border p-4 text-left"
              onclick={() => {
                sheet.open = true;
              }}
              data-testid="open-features"
            >
              <span
                class="bg-primary/12 text-primary flex size-11 shrink-0 items-center justify-center rounded-2xl"
              >
                <SlidersHorizontal class="size-5" aria-hidden="true" />
              </span>
              <span class="flex min-w-0 flex-1 flex-col">
                <span class="font-semibold tracking-tight">All features</span>
                <span class="text-muted-foreground text-sm">
                  {entities.length} features{device?.ever_writable === true && changeable > 0
                    ? ` · ${String(changeable)} you can change`
                    : ''}
                </span>
              </span>
              <ChevronRight class="text-muted-foreground size-5 shrink-0" aria-hidden="true" />
            </button>
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>

<FeatureSheet
  open={sheet.open}
  onopenchange={(open) => {
    sheet.open = open;
  }}
  {api}
  {entities}
  {programs}
  {slug}
  role={device?.role ?? ''}
  write={device?.ever_writable === true ? write : null}
/>
