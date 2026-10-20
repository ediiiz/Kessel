<script lang="ts">
  /**
   * The appliance cards on the home screen: name and connection state above, the role's artwork
   * below. A swipeable row on a phone, a grid from `sm`.
   *
   * Every card is a link and nothing else; writes live on the device page. Nothing here renders a
   * secret identifier: `DeviceSummary` carries only the role, slug, brand and model.
   */
  import { resolve } from '$app/paths';
  import { Flame, Lock, Plus } from '$lib/icons';
  import type { DeviceSummary } from '$lib/api-types';
  import { ROUTES } from '$lib/index.js';
  import {
    connectionDetail,
    connectionLook,
    isHeating,
    modelLine,
    roleLabel,
    type Tone,
  } from '$lib/appliance';
  import ApplianceArt from './ApplianceArt.svelte';
  import { enter, flipMs } from '$lib/motion';
  import { flip } from 'svelte/animate';
  import { cn } from '$lib/utils.js';

  const { devices, onadd }: { devices: DeviceSummary[]; onadd?: () => void } = $props();

  const TONE: Record<Tone, string> = {
    live: 'text-success',
    working: 'text-warning',
    idle: 'text-muted-foreground',
    bad: 'text-destructive',
  };
</script>

{#if devices.length === 0}
  <div
    class="glass flex flex-col items-center gap-2 rounded-3xl border px-6 py-12 text-center"
    data-testid="no-devices"
  >
    <p class="text-lg font-semibold tracking-tight">No appliances yet</p>
    <p class="text-muted-foreground max-w-sm text-sm">Add one with its profile ZIP.</p>
    {#if onadd !== undefined}
      <button
        type="button"
        onclick={onadd}
        data-testid="add-appliance-empty"
        class="press bg-primary text-primary-foreground hover:bg-primary/90 mt-3 inline-flex h-10 items-center gap-1.5 rounded-full px-5 text-sm font-medium shadow-[0_8px_24px_-8px_var(--primary)]"
      >
        <Plus class="size-4" aria-hidden="true" />
        Add appliance
      </button>
    {/if}
  </div>
{:else}
  <ul
    class="no-scrollbar scroll-fade-x sm:scroll-fade-none -mx-4 flex snap-x snap-mandatory gap-3 overflow-x-auto px-4 pb-2 sm:mx-0 sm:grid sm:grid-cols-2 sm:overflow-visible sm:px-0 lg:grid-cols-3"
  >
    {#each devices as device, index (device.slug)}
      {@const look = connectionLook(device.connection_state)}
      {@const detail = connectionDetail(device.connection_state, device.last_failure)}
      <li
        class="w-[72%] shrink-0 snap-start sm:w-auto"
        data-testid="device-row"
        data-slug={device.slug}
        in:enter|global={{ index }}
        animate:flip={{ duration: flipMs() }}
      >
        <a
          data-testid="device-link"
          href={resolve(ROUTES.device, { slug: device.slug })}
          class="group press glass focus-visible:ring-ring/50 flex h-full flex-col overflow-hidden rounded-3xl border hover:-translate-y-0.5 focus-visible:ring-[3px] focus-visible:outline-none"
        >
          <div class="flex flex-col gap-0.5 px-4 pt-4 pb-3">
            <p class="truncate text-lg font-semibold tracking-tight">{roleLabel(device.role)}</p>
            <p class="flex min-w-0 items-center gap-1.5 text-sm">
              <span
                class={cn('font-medium capitalize', TONE[look.tone])}
                data-testid="device-connection-state">{device.connection_state}</span
              >
              {#if detail !== null}
                <span
                  class="text-muted-foreground truncate text-xs"
                  data-testid="device-connection-detail"
                  title={detail}>· {detail}</span
                >
              {/if}
            </p>
          </div>

          <div class="relative">
            <ApplianceArt
              role={device.role}
              class="h-36 transition-transform duration-(--motion-base) group-hover:scale-[1.03]"
            />
            <div
              class="absolute inset-x-3 bottom-3 z-30 flex items-center gap-1.5 text-[0.7rem] text-white/90"
            >
              <span
                class="truncate rounded-full bg-black/25 px-2 py-0.5 font-mono backdrop-blur-md"
              >
                <span data-testid="device-role">{device.role}</span
                >{#if modelLine(device) !== null}<span data-testid="device-model"
                    >&nbsp;· {modelLine(device)}</span
                  >{/if}
              </span>
              <span class="ml-auto shrink-0 rounded-full bg-black/25 px-2 py-0.5 backdrop-blur-md">
                <span data-testid="device-entity-count">{device.entity_count}</span> features
              </span>
              {#if isHeating(device.role)}
                <span class="rounded-full bg-black/25 p-1 backdrop-blur-md" title="Produces heat">
                  <Flame class="size-3" aria-hidden="true" />
                </span>
              {/if}
              {#if !device.ever_writable}
                <span
                  class="rounded-full bg-black/25 p-1 backdrop-blur-md"
                  title="Read-only"
                  data-testid="never-writable"
                >
                  <Lock class="size-3" aria-hidden="true" />
                </span>
              {/if}
            </div>
          </div>
        </a>
      </li>
    {/each}
  </ul>
{/if}
