<script lang="ts">
  /**
   * The running or selected programme, its time remaining and its progress. Read-only: Kessel shows
   * the programme, it does not start one.
   */
  import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
  import { Timer } from '$lib/icons';
  import { displayValue, progressPercent } from '$lib/value';
  import { enter, flash } from '$lib/motion';

  const {
    entities,
    programs,
    index = 0,
  }: { entities: EntityDescriptor[]; programs: ProgramDescriptor[]; index?: number } = $props();

  function find(suffix: string): EntityDescriptor | undefined {
    return entities.find((entity) => entity.feature_key?.endsWith(suffix) === true);
  }

  const active = $derived(find('Root.ActiveProgram'));
  const selected = $derived(find('Root.SelectedProgram'));
  const remaining = $derived(find('Option.RemainingProgramTime'));
  const progress = $derived(find('Option.ProgramProgress'));
  const operation = $derived(find('Status.OperationState'));

  const running = $derived(
    active !== undefined && active.value !== null && active.value !== '0' && active.value !== '',
  );
  const programme = $derived(running ? active : selected);
  const name = $derived(
    programme === undefined ? null : displayValue(programme.feature_key, programme.value, programs),
  );
  const percent = $derived(running ? progressPercent(progress?.value ?? null) : null);
  const time = $derived(
    remaining === undefined ? null : displayValue(remaining.feature_key, remaining.value),
  );
</script>

{#if programme !== undefined}
  <section
    class="glass flex flex-col gap-5 rounded-3xl border p-5"
    data-testid="programme-card"
    in:enter|global={{ index }}
  >
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <p class="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          {running ? 'Running' : 'Selected programme'}
        </p>
        <p class="mt-1 truncate text-2xl font-semibold tracking-tight" data-testid="programme-name">
          {name}
        </p>
      </div>
      {#if operation !== undefined}
        <span
          class="bg-primary/10 text-primary shrink-0 rounded-full px-3 py-1 text-xs font-semibold"
          data-testid="programme-operation"
        >
          {displayValue(operation.feature_key, operation.value)}
        </span>
      {/if}
    </div>

    {#if time !== null}
      <div class="flex items-end justify-between gap-3">
        <p class="text-muted-foreground flex items-center gap-1.5 text-sm">
          <Timer class="size-4" aria-hidden="true" />
          {running ? 'Time remaining' : 'Duration'}
        </p>
        <p
          class="text-4xl font-semibold tracking-tight tabular-nums"
          data-testid="programme-remaining"
        >
          {#key time}<span class="rounded-md" in:flash>{time}</span>{/key}
        </p>
      </div>
    {/if}

    {#if percent !== null}
      <div
        class="bg-muted h-2 overflow-hidden rounded-full"
        role="progressbar"
        aria-valuenow={percent}
        aria-valuemin="0"
        aria-valuemax="100"
        aria-label="Progress {String(percent)}%"
      >
        <div
          class="from-primary h-full rounded-full bg-gradient-to-r to-[oklch(0.75_0.15_200)] transition-[width] duration-700"
          style:width="{percent}%"
        ></div>
      </div>
    {/if}
  </section>
{/if}
