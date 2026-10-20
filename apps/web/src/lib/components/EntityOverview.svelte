<script lang="ts">
  /**
   * The at-a-glance state: the features listed in `OVERVIEW_KEYS`, as small tiles. A feature that
   * is exposed but has not reported shows an em dash. Values flash when the live stream changes
   * them.
   */
  import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
  import { OVERVIEW_KEYS } from '$lib/appliance';
  import { displayValue, progressPercent, readsAsOn } from '$lib/value';
  import { enter, flash } from '$lib/motion';
  import { cn } from '$lib/utils.js';

  const {
    entities,
    programs = [],
    exclude = [],
    index = 0,
  }: {
    entities: EntityDescriptor[];
    /** Names a `SelectedProgram` / `ActiveProgram` value. */
    programs?: ProgramDescriptor[];
    /** Key suffixes shown elsewhere on the page. */
    exclude?: readonly string[];
    /** Position in the page's entrance sequence. See `$lib/motion.ts`. */
    index?: number;
  } = $props();

  interface Tile {
    readonly label: string;
    readonly key: string;
    readonly value: string;
    readonly percent: number | null;
    readonly on: boolean;
  }

  /**
   * One tile per overview key that this appliance actually exposes, in `OVERVIEW_KEYS` order.
   *
   * A feature the profile does not expose produces no tile — the section simply gets shorter. A
   * feature that is exposed but has no reported value **does** get a tile, showing an em dash: "the
   * appliance has not told us" is information, and hiding the tile would render it as "this appliance
   * has no power switch".
   */
  const tiles = $derived(
    OVERVIEW_KEYS.flatMap(({ suffix, label, zero }): Tile[] => {
      if (exclude.includes(suffix)) {
        return [];
      }
      const match = entities.find((entity) => entity.feature_key?.endsWith(suffix) === true);
      if (match === undefined) {
        return [];
      }
      const raw = match.value;
      return [
        {
          label,
          key: match.feature_key ?? '',
          value:
            zero !== undefined && raw === '0'
              ? zero
              : displayValue(match.feature_key, raw, programs),
          // Read separately from the label because this one drives a bar as well as a string, and the
          // bar must not appear for a value that is not a percentage. `progressPercent` returns
          // `null` for anything outside 0–100, which is also what `displayValue` keys off.
          percent: suffix.endsWith('ProgramProgress') ? progressPercent(raw) : null,
          on: readsAsOn(raw),
        },
      ];
    }),
  );
</script>

{#if tiles.length > 0}
  <section class="flex flex-col gap-3" data-testid="entity-overview" in:enter|global={{ index }}>
    <h2 class="text-lg font-semibold tracking-tight">Status</h2>
    <ul class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
      {#each tiles as tile (tile.key)}
        <li
          class="bg-surface-strong backdrop-blur-md flex flex-col gap-1 rounded-2xl border p-3.5 shadow-(--shadow-soft)"
          data-testid="overview-tile"
          data-key={tile.key}
        >
          <p class="text-muted-foreground truncate text-xs font-medium" title={tile.key}>
            {tile.label}
          </p>
          <p
            class={cn(
              'truncate text-lg font-semibold tracking-tight',
              tile.on && (tile.key.endsWith('DoorState') ? 'text-warning' : 'text-primary'),
            )}
            data-testid="overview-value"
            title={tile.value}
          >
            {#key tile.value}
              <span class="rounded-sm" in:flash>{tile.value}</span>
            {/key}
          </p>
          {#if tile.percent !== null}
            <div
              class="bg-muted mt-1 h-1 overflow-hidden rounded-full"
              role="progressbar"
              aria-valuenow={tile.percent}
              aria-valuemin="0"
              aria-valuemax="100"
              aria-label="{tile.label} {String(tile.percent)}%"
            >
              <div
                class="bg-primary h-full rounded-full transition-[width] duration-500"
                style="width: {tile.percent}%"
              ></div>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}
