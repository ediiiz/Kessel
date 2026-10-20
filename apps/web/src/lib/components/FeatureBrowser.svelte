<script lang="ts">
  /**
   * Every feature the appliance declares, findable and changeable without knowing its key.
   *
   * Search matches the name, the key and the value. The filter defaults to the features that
   * report something, which is what "what is it doing" means; Changeable and All are a tap away.
   * Rows group by section (Settings, Options, Status…). A change is proposed by its row and sent
   * only from the confirmation here, so this is the one path every catalogue write takes.
   */
  import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
  import * as Empty from '$lib/components/ui/empty';
  import * as InputGroup from '$lib/components/ui/input-group';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import {
    FEATURE_SECTIONS,
    featureSection,
    namesakeHints,
    type FeatureSection,
  } from '$lib/features';
  import { Search } from '$lib/icons';
  import { blankValue } from '$lib/value';
  import { WriteGate, type Writer } from '$lib/write.svelte';
  import FeatureRow from './FeatureRow.svelte';
  import WriteConfirmation from './WriteConfirmation.svelte';

  type Filter = 'reported' | 'writable' | 'all';

  const {
    entities,
    programs = [],
    slug,
    role,
    write,
  }: {
    entities: EntityDescriptor[];
    programs?: readonly ProgramDescriptor[];
    slug: string;
    role: string;
    /** `null` for an appliance that takes no writes: every row is then read-only. */
    write: Writer | null;
  } = $props();

  const gate = new WriteGate();
  const view = $state<{ query: string; filter: Filter | null }>({ query: '', filter: null });

  const reported = (entity: EntityDescriptor): boolean => blankValue(entity.value) !== '—';
  const counts = $derived({
    reported: entities.filter((entity) => reported(entity)).length,
    writable: entities.filter((entity) => entity.writable).length,
    all: entities.length,
  });
  /** Until the operator picks, "In use" — unless nothing reports yet, as on an offline appliance. */
  const filter = $derived<Filter>(view.filter ?? (counts.reported > 0 ? 'reported' : 'all'));

  const hints = $derived(namesakeHints(entities));

  const sections = $derived.by(() => {
    const needle = view.query.trim().toLowerCase();
    const grouped = new Map<FeatureSection, EntityDescriptor[]>();
    for (const entity of entities) {
      if (filter === 'reported' && !reported(entity)) {
        continue;
      }
      if (filter === 'writable' && !entity.writable) {
        continue;
      }
      const haystack = [
        entity.label,
        hints.get(entity.uid) ?? null,
        entity.feature_key,
        entity.value,
        String(entity.uid),
      ];
      if (
        needle !== '' &&
        !haystack.some((field) => field?.toLowerCase().includes(needle) === true)
      ) {
        continue;
      }
      const section = featureSection(entity.feature_key);
      grouped.set(section, [...(grouped.get(section) ?? []), entity]);
    }
    return FEATURE_SECTIONS.flatMap((section) => {
      const rows = grouped.get(section);
      return rows === undefined
        ? []
        : [{ section, rows: rows.toSorted((a, b) => a.label.localeCompare(b.label)) }];
    });
  });

  function editQuery(event: Event): void {
    const input = event.currentTarget;
    view.query = input instanceof HTMLInputElement ? input.value : view.query;
  }

  function pick(next: string): void {
    if (next === 'reported' || next === 'writable' || next === 'all') {
      view.filter = next;
    }
  }

  const FILTERS: readonly { id: Filter; label: string }[] = [
    { id: 'reported', label: 'In use' },
    { id: 'writable', label: 'Changeable' },
    { id: 'all', label: 'All' },
  ];
</script>

<div class="flex min-h-0 flex-1 flex-col gap-3" data-testid="feature-browser">
  <div class="flex flex-col gap-3">
    <InputGroup.Root class="bg-surface-strong h-11 rounded-2xl">
      <InputGroup.Addon>
        <Search aria-hidden="true" />
      </InputGroup.Addon>
      <InputGroup.Input
        type="search"
        placeholder="Search features"
        aria-label="Search features"
        value={view.query}
        oninput={editQuery}
        data-testid="feature-search"
      />
    </InputGroup.Root>

    <ToggleGroup.Root
      type="single"
      spacing={2}
      value={filter}
      onValueChange={pick}
      class="flex flex-wrap justify-start gap-2"
      aria-label="Show"
    >
      {#each FILTERS as option (option.id)}
        <ToggleGroup.Item
          value={option.id}
          data-testid="feature-filter-{option.id}"
          class="press bg-surface-strong data-[state=on]:border-primary data-[state=on]:bg-primary/12 data-[state=on]:text-primary h-9 gap-2 rounded-full border px-3.5 text-sm"
        >
          {option.label}
          <span class="text-muted-foreground text-xs tabular-nums">{counts[option.id]}</span>
        </ToggleGroup.Item>
      {/each}
    </ToggleGroup.Root>
  </div>

  <div
    class="no-scrollbar scroll-fade -mx-4 flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 pt-1 pb-[calc(1.5rem+env(safe-area-inset-bottom))] sm:-mx-6 sm:px-6"
    data-testid="feature-scroller"
  >
    {#if sections.length === 0}
      <Empty.Root class="rounded-3xl border border-dashed" data-testid="feature-empty">
        <Empty.Header>
          <Empty.Media variant="icon"><Search aria-hidden="true" /></Empty.Media>
          <Empty.Title>Nothing matches</Empty.Title>
          <Empty.Description
            >Try another search, or show all {counts.all} features.</Empty.Description
          >
        </Empty.Header>
      </Empty.Root>
    {:else}
      {#each sections as group (group.section)}
        <section
          class="flex flex-col gap-2"
          data-testid="feature-section"
          data-section={group.section}
        >
          <h3 class="text-muted-foreground px-1 text-xs font-semibold tracking-wide uppercase">
            {group.section}
            <span class="font-normal tabular-nums">· {group.rows.length}</span>
          </h3>
          <ul class="glass divide-border/50 divide-y overflow-hidden rounded-3xl border">
            {#each group.rows as entity (entity.uid)}
              <FeatureRow
                {entity}
                hint={hints.get(entity.uid) ?? null}
                {programs}
                canWrite={write !== null}
                busy={gate.busy}
                propose={(proposed) => {
                  gate.propose(proposed);
                }}
              />
            {/each}
          </ul>
        </section>
      {/each}
    {/if}
  </div>
</div>

{#if write !== null}
  <WriteConfirmation {gate} {slug} {role} {write} />
{/if}
