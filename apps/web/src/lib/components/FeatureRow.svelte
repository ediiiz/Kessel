<script lang="ts">
  /**
   * One feature in the browser: its name and value, and on tap the details and, when it can be
   * changed, the control that changes it. An enumeration gets its members as chips; anything else a
   * value field. Either way the row only proposes: the browser's confirmation is what sends.
   */
  import { slide } from 'svelte/transition';
  import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
  import { optionLabel } from '$lib/appliance';
  import { ChevronDown, Lock, Pencil } from '$lib/icons';
  import { reducedMotion } from '$lib/motion';
  import { blankValue, displayValue, programName } from '$lib/value';
  import type { ProposedWrite } from '$lib/write.svelte';
  import { cn } from '$lib/utils.js';

  const {
    entity,
    hint,
    programs,
    canWrite,
    busy,
    propose,
  }: {
    entity: EntityDescriptor;
    /** What separates this feature from another of the same name, or `null` when it has none. */
    hint: string | null;
    /** Names a programme uid in the value of `Root.SelectedProgram` and its kin. */
    programs: readonly ProgramDescriptor[];
    /** False when the appliance takes no writes at all; every row is then read-only. */
    canWrite: boolean;
    /** A write is in flight, so nothing else may be proposed. */
    busy: boolean;
    propose: (write: ProposedWrite) => void;
  } = $props();

  const row = $state({ open: false, draft: '' });

  const key = $derived(entity.feature_key);
  const editable = $derived(canWrite && entity.writable && key !== null);
  const isOption = $derived(entity.options.length > 0);
  /** A member's name: the programme it selects, its contributed label, or the member re-spelled. */
  const name = (member: string): string =>
    programName(member, programs) ?? optionLabel(entity.option_labels, member);
  const shown = $derived(
    isOption && entity.value !== null && entity.options.includes(entity.value)
      ? name(entity.value)
      : displayValue(key, entity.value, programs),
  );
  const duration = $derived(reducedMotion() ? 0 : 180);

  function toggle(): void {
    row.open = !row.open;
  }

  function editDraft(event: Event): void {
    const input = event.currentTarget;
    row.draft = input instanceof HTMLInputElement ? input.value : row.draft;
  }

  function choose(member: string): void {
    if (key === null) {
      return;
    }
    propose({
      featureKey: key,
      value: member,
      label: `${entity.label} → ${name(member)}`,
    });
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    const value = row.draft.trim();
    if (key === null || value === '') {
      return;
    }
    propose({ featureKey: key, value, label: `${entity.label} → ${value}` });
  }
</script>

<li data-testid="feature-row" data-key={key ?? ''} data-uid={entity.uid}>
  <button
    type="button"
    class="press hover:bg-foreground/[0.03] flex w-full items-center gap-3 px-4 py-3 text-left"
    aria-expanded={row.open}
    onclick={toggle}
    data-testid="feature-toggle"
  >
    <span class="flex min-w-0 flex-1 flex-col">
      <span class="flex min-w-0 items-center gap-1.5">
        <span class="truncate text-sm font-medium" data-testid="feature-label">{entity.label}</span>
        {#if editable}
          <Pencil class="text-primary size-3 shrink-0" aria-label="Can be changed" />
        {/if}
      </span>
      {#if hint !== null}
        <span class="text-muted-foreground truncate text-xs" data-testid="feature-hint">{hint}</span
        >
      {/if}
    </span>
    <span
      class={cn(
        'max-w-[45%] truncate text-sm tabular-nums',
        shown === '—' ? 'text-muted-foreground/60' : 'text-muted-foreground',
      )}
      data-testid="feature-value">{shown}</span
    >
    <ChevronDown
      class={cn(
        'text-muted-foreground/60 size-4 shrink-0 transition-transform',
        row.open && 'rotate-180',
      )}
      aria-hidden="true"
    />
  </button>

  {#if row.open}
    <div
      class="flex flex-col gap-3 px-4 pb-4"
      transition:slide={{ duration }}
      data-testid="feature-detail"
    >
      {#if entity.description !== null}
        <p class="text-muted-foreground text-sm">{entity.description}</p>
      {/if}

      {#if editable && isOption}
        <div class="flex flex-wrap gap-2" role="group" aria-label="Set {entity.label}">
          {#each entity.options as member (member)}
            {@const current = member === entity.value}
            <button
              type="button"
              class={cn(
                'press h-9 rounded-full border px-3.5 text-sm font-medium disabled:opacity-50',
                current
                  ? 'border-primary bg-primary/12 text-primary disabled:opacity-100'
                  : 'bg-surface-strong hover:border-primary/50',
              )}
              aria-pressed={current}
              disabled={busy || current}
              onclick={() => {
                choose(member);
              }}
              data-testid="feature-option"
              data-value={member}
            >
              {name(member)}
            </button>
          {/each}
        </div>
      {:else if editable}
        <form class="flex gap-2" onsubmit={submit}>
          <input
            type="text"
            autocomplete="off"
            spellcheck="false"
            aria-label="New value for {entity.label}"
            placeholder={blankValue(entity.value) === '—' ? 'Value' : blankValue(entity.value)}
            value={row.draft}
            oninput={editDraft}
            class="border-input bg-background/70 focus-visible:border-ring focus-visible:ring-ring/50 h-10 min-w-0 flex-1 rounded-xl border px-3 font-mono text-sm focus-visible:ring-[3px] focus-visible:outline-none"
            data-testid="feature-input"
          />
          <button
            type="submit"
            class="press bg-primary text-primary-foreground h-10 shrink-0 rounded-xl px-4 text-sm font-medium disabled:opacity-50"
            disabled={busy || row.draft.trim() === ''}
            data-testid="feature-set"
          >
            Set
          </button>
        </form>
      {:else}
        <p
          class="text-muted-foreground flex items-center gap-1.5 text-xs"
          data-testid="feature-readonly"
        >
          <Lock class="size-3" aria-hidden="true" />
          Read only
        </p>
      {/if}

      <dl
        class="text-muted-foreground grid grid-cols-[max-content_1fr] gap-x-3 gap-y-0.5 text-[0.7rem]"
      >
        <dt>Key</dt>
        <dd class="text-foreground/80 font-mono break-all" data-testid="feature-key">
          {key ?? '(unnamed)'}
        </dd>
        <dt>Raw value</dt>
        <dd class="font-mono break-all">{blankValue(entity.value)}</dd>
        <dt>Access</dt>
        <dd class="font-mono">{entity.access ?? '—'} · uid {entity.uid}</dd>
      </dl>
    </div>
  {/if}
</li>
