<script lang="ts">
  /**
   * What this appliance can run, as a swipeable row. Read-only: nothing here starts a programme.
   */
  import type { ProgramDescriptor } from '$lib/api-types';
  import { ListOrdered } from '$lib/icons';
  import { enter } from '$lib/motion';

  const {
    programs,
    index = 0,
  }: {
    programs: ProgramDescriptor[];
    /** Position in the page's entrance sequence. See `$lib/motion.ts`. */
    index?: number;
  } = $props();
</script>

{#if programs.length > 0}
  <section class="flex flex-col gap-3" data-testid="program-list" in:enter|global={{ index }}>
    <div class="flex items-baseline gap-2">
      <h2 class="text-lg font-semibold tracking-tight">Programmes</h2>
      <span class="text-muted-foreground text-sm">{programs.length}</span>
    </div>
    <ul
      class="no-scrollbar scroll-fade-x md:scroll-fade-none -mx-4 flex snap-x scroll-px-4 gap-2.5 overflow-x-auto px-4 pb-1 md:mx-0 md:flex-wrap md:scroll-px-0 md:px-0"
    >
      {#each programs as program (program.uid)}
        <li
          class="bg-surface-strong backdrop-blur-md flex w-40 shrink-0 snap-start flex-col gap-1 rounded-2xl border p-3.5 shadow-(--shadow-soft)"
          data-testid="program"
          data-uid={program.uid}
          title={program.feature_key ?? undefined}
        >
          <ListOrdered class="text-primary size-4" aria-hidden="true" />
          <p class="mt-2 line-clamp-2 text-sm font-semibold" data-testid="program-label">
            {program.label}
          </p>
          {#if program.description !== null}
            <p class="text-muted-foreground line-clamp-2 text-xs">{program.description}</p>
          {/if}
          {#if program.execution !== null}
            <span
              class="text-muted-foreground/80 mt-auto pt-1 font-mono text-[0.65rem]"
              data-testid="program-execution">{program.execution}</span
            >
          {/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}
