<script lang="ts">
  /**
   * Two-state settings as tiles: every writable feature with exactly two declared members. A tap
   * sends the other declared member. Until the appliance has reported which one is current, the
   * tile offers both instead of guessing.
   *
   * On an appliance that does not produce heat, a tap applies the setting straight away; on an oven
   * or a cooktop it asks first. The pressed state always follows the value the appliance last
   * reported, so a refused write leaves the control where the appliance actually is.
   */
  import { Lightbulb, Lock, Power, Timer, ToggleRight, VolumeX, Zap } from '$lib/icons';
  import type { EntityDescriptor } from '$lib/api-types';
  import { isHeating, optionLabel } from '$lib/appliance';
  import { WriteGate, type Writer } from '$lib/write.svelte';
  import { enter } from '$lib/motion';
  import { readsAsOn } from '$lib/value';
  import { cn } from '$lib/utils.js';
  import WriteConfirmation from './WriteConfirmation.svelte';

  const {
    entities,
    slug,
    role,
    write,
    index = 0,
  }: {
    entities: EntityDescriptor[];
    slug: string;
    role: string;
    write: Writer;
    /** Position in the page's entrance sequence. See `$lib/motion.ts`. */
    index?: number;
  } = $props();

  const gate = new WriteGate();
  const confirmFirst = $derived(isHeating(role));

  function choose(tile: EntityDescriptor, option: string): void {
    const proposed = {
      featureKey: tile.feature_key ?? '',
      value: option,
      label: `${tile.label} → ${optionLabel(tile.option_labels, option)}`,
    };
    if (confirmFirst) {
      gate.propose(proposed);
    } else {
      void gate.send(slug, write, proposed);
    }
  }

  /** Writable, two declared members, and a key to send. A feature with no key cannot be written. */
  const tiles = $derived(
    entities.filter(
      (entity) => entity.writable && entity.options.length === 2 && entity.feature_key !== null,
    ),
  );

  /** The other declared member, or `null` when the current value is neither. */
  function other(tile: EntityDescriptor): string | null {
    const [first, second] = tile.options;
    if (tile.value === first) return second ?? null;
    if (tile.value === second) return first ?? null;
    return null;
  }

  /** Emphasis only: whether the tile reads as switched on. */
  function lit(value: string | null): boolean {
    return readsAsOn(value) || (value?.endsWith('On') ?? false);
  }

  const GLYPHS: readonly [RegExp, typeof Power][] = [
    [/PowerState$/u, Power],
    [/(Silence|Quiet|Sound|Volume)/u, VolumeX],
    [/(Light|Lamp|Beam)/u, Lightbulb],
    [/(ChildLock|Lock)/u, Lock],
    [/(Time|Delay|Start)/u, Timer],
  ];

  function glyph(key: string): typeof Power {
    return GLYPHS.find(([pattern]) => pattern.test(key))?.[1] ?? ToggleRight;
  }
</script>

{#if tiles.length > 0}
  <section class="flex flex-col gap-3" data-testid="quick-actions" in:enter|global={{ index }}>
    <div class="flex items-center gap-2">
      <Zap class="text-primary size-4 shrink-0" aria-hidden="true" />
      <h2 class="text-lg font-semibold tracking-tight">Quick actions</h2>
    </div>

    <ul class="grid grid-cols-2 gap-3 lg:grid-cols-3">
      {#each tiles as tile (tile.uid)}
        {@const key = tile.feature_key ?? ''}
        {@const next = other(tile)}
        {@const on = lit(tile.value)}
        {@const sending = gate.sending?.featureKey === key}
        {@const Glyph = glyph(key)}
        <li data-testid="quick-action" data-uid={tile.uid}>
          {#if next !== null}
            <button
              type="button"
              data-testid="quick-action-toggle"
              data-value={next}
              aria-pressed={on}
              aria-busy={sending}
              disabled={gate.busy}
              title={key}
              onclick={() => choose(tile, next)}
              class={cn(
                'press relative flex h-full min-h-28 w-full flex-col justify-between gap-3 overflow-hidden rounded-3xl border p-4 text-left disabled:cursor-wait',
                on
                  ? 'border-transparent bg-gradient-to-br from-[oklch(0.62_0.19_258)] to-[oklch(0.55_0.2_280)] text-white shadow-[0_12px_30px_-12px_oklch(0.55_0.2_265)]'
                  : 'bg-surface-strong backdrop-blur-md hover:bg-card shadow-(--shadow-soft)',
                sending && 'animate-pulse',
              )}
            >
              <span class="flex items-start justify-between gap-2">
                <span class="text-lg leading-tight font-semibold" data-testid="quick-action-value">
                  {tile.value === null ? '—' : optionLabel(tile.option_labels, tile.value)}
                </span>
                <span
                  class={cn(
                    'flex size-9 shrink-0 items-center justify-center rounded-full',
                    on ? 'bg-white/20' : 'bg-muted text-muted-foreground',
                  )}
                >
                  <Glyph class="size-4" aria-hidden="true" />
                </span>
              </span>
              <span
                class={cn('line-clamp-2 text-sm', on ? 'text-white/85' : 'text-muted-foreground')}
              >
                {tile.label}
              </span>
            </button>
          {:else}
            <div
              class="bg-surface-strong backdrop-blur-md flex h-full min-h-28 flex-col justify-between gap-3 rounded-3xl border p-4 shadow-(--shadow-soft)"
            >
              <span class="text-muted-foreground line-clamp-2 text-sm" title={key}
                >{tile.label}</span
              >
              <span class="bg-muted grid grid-cols-2 gap-1 rounded-full p-1">
                {#each tile.options as option (option)}
                  <button
                    type="button"
                    data-testid="quick-action-option"
                    data-value={option}
                    aria-pressed={false}
                    disabled={gate.busy}
                    onclick={() => choose(tile, option)}
                    class="press hover:bg-selected truncate rounded-full px-2 py-1 text-xs font-medium"
                  >
                    {optionLabel(tile.option_labels, option)}
                  </button>
                {/each}
              </span>
            </div>
          {/if}
        </li>
      {/each}
    </ul>

    {#if confirmFirst}
      <WriteConfirmation {gate} {slug} {role} {write} />
    {/if}
  </section>
{/if}
