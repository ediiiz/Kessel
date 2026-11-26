<script lang="ts">
  /**
   * The running or selected programme, its time remaining and its progress, with a programme
   * dropdown, Start for the selected programme and Stop for the running one.
   *
   * Every control is proposed as a write to a programme pointer (`$lib/programme`), so what is
   * confirmed is what changes, and goes through the same {@link WriteGate} as every other write:
   * confirmed first on an appliance that produces heat, sent at once elsewhere. Kessel does not judge
   * whether the appliance should select, start or stop; its answer is the toast. Without a `write`
   * callback the card is read-only.
   */
  import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
  import { CircleStop, Play, Timer } from '$lib/icons';
  import { isHeating } from '$lib/appliance';
  import * as Select from '$lib/components/ui/select';
  import { ACTIVE_PROGRAM, NO_PROGRAMME, SELECTED_PROGRAM, programmeRuns } from '$lib/programme';
  import { displayValue, programFor, progressPercent } from '$lib/value';
  import { enter, flash } from '$lib/motion';
  import { WriteGate, type ProposedWrite, type Writer } from '$lib/write.svelte';
  import WriteConfirmation from './WriteConfirmation.svelte';

  const {
    entities,
    programs,
    slug = '',
    role = '',
    write,
    index = 0,
  }: {
    entities: EntityDescriptor[];
    programs: ProgramDescriptor[];
    slug?: string;
    role?: string;
    /**
     * Sends a programme action as a write to a pointer (`$lib/programme.programmeWriter`). Absent makes
     * the card read-only.
     */
    write?: Writer;
    index?: number;
  } = $props();

  function find(suffix: string): EntityDescriptor | undefined {
    return entities.find((entity) => entity.feature_key?.endsWith(suffix) === true);
  }

  const active = $derived(find('Root.ActiveProgram'));
  const selected = $derived(find('Root.SelectedProgram'));
  const remaining = $derived(find('Option.RemainingProgramTime'));
  const progress = $derived(find('Option.ProgramProgress'));
  const operation = $derived(find('Status.OperationState'));

  const running = $derived(programmeRuns(operation?.value, active?.value));
  const programme = $derived(running ? active : selected);
  const name = $derived(
    programme === undefined ? null : displayValue(programme.feature_key, programme.value, programs),
  );
  const percent = $derived(running ? progressPercent(progress?.value ?? null) : null);
  const time = $derived(
    remaining === undefined ? null : displayValue(remaining.feature_key, remaining.value),
  );

  /** The programmes this profile names by key: the only ones a write can address. */
  const named = $derived(programs.filter((program) => program.feature_key !== null));
  /** The selected programme, when the appliance reports one this profile names by key. */
  const current = $derived(
    named.find((program) => program === programFor(selected?.value ?? null, programs)) ?? null,
  );
  const selectable = $derived(selected?.writable === true && named.length > 0);

  const gate = new WriteGate();
  const confirmFirst = $derived(isHeating(role));

  function propose(send: Writer, proposed: ProposedWrite): void {
    if (confirmFirst) {
      gate.propose(proposed);
    } else {
      void gate.send(slug, send, proposed);
    }
  }

  function choose(send: Writer, key: string): void {
    const program = named.find((candidate) => candidate.feature_key === key);
    if (program === undefined || program === current) {
      return;
    }
    propose(send, {
      featureKey: selected?.feature_key ?? SELECTED_PROGRAM,
      value: key,
      label: `Select ${program.label}`,
    });
  }
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

    {#if write !== undefined}
      {@const send = write}
      <div class="flex flex-col gap-2 sm:flex-row" data-testid="programme-controls">
        {#if selectable}
          <Select.Root
            type="single"
            value={current?.feature_key ?? ''}
            onValueChange={(key) => choose(send, key)}
          >
            <Select.Trigger
              data-testid="programme-select"
              aria-label="Programme"
              disabled={gate.busy}
              class="bg-surface-strong h-11 w-full rounded-2xl sm:flex-1"
            >
              {current?.label ?? 'Choose a programme'}
            </Select.Trigger>
            <Select.Content>
              <Select.Group>
                {#each named as program (program.uid)}
                  <Select.Item value={program.feature_key ?? ''} label={program.label}>
                    {program.label}
                  </Select.Item>
                {/each}
              </Select.Group>
            </Select.Content>
          </Select.Root>
        {/if}

        {#if running}
          <button
            type="button"
            data-testid="programme-stop"
            disabled={gate.busy}
            aria-busy={gate.sending !== null}
            onclick={() => {
              propose(send, {
                featureKey: active?.feature_key ?? ACTIVE_PROGRAM,
                value: NO_PROGRAMME,
                label: `Stop ${name ?? 'programme'}`,
              });
            }}
            class="press bg-destructive hover:bg-destructive/90 inline-flex h-11 items-center justify-center gap-2 rounded-2xl px-4 text-sm font-semibold text-white disabled:cursor-wait disabled:opacity-60"
          >
            <CircleStop class="size-4" aria-hidden="true" />
            {gate.sending === null ? `Stop ${name ?? 'programme'}` : 'Stopping…'}
          </button>
        {:else if current !== null}
          {@const program = current}
          <button
            type="button"
            data-testid="programme-start"
            disabled={gate.busy}
            aria-busy={gate.sending !== null}
            title={program.feature_key}
            onclick={() => {
              propose(send, {
                featureKey: active?.feature_key ?? ACTIVE_PROGRAM,
                value: program.feature_key ?? '',
                label: `Start ${program.label}`,
              });
            }}
            class="press bg-primary text-primary-foreground hover:bg-primary/90 inline-flex h-11 items-center justify-center gap-2 rounded-2xl px-4 text-sm font-semibold disabled:cursor-wait disabled:opacity-60"
          >
            <Play class="size-4" aria-hidden="true" />
            {gate.sending === null ? `Start ${program.label}` : 'Starting…'}
          </button>
        {/if}
      </div>
    {/if}

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

    {#if write !== undefined && confirmFirst}
      <WriteConfirmation {gate} {slug} {role} {write} />
    {/if}
  </section>
{/if}
