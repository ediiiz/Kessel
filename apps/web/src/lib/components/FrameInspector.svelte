<script lang="ts">
  /**
   * The live frame inspector.
   *
   * It renders what the daemon sends and nothing else. `TraceFrame` carries `seq`, `kind`, `level`,
   * `source`, `target`, `message` and a `fields` map of already-rendered strings; the direction, the
   * decoded body, the handshake step and the chain state all arrive inside `fields` (a frame event is
   * one with a `direction` field). Every field is rendered verbatim and nothing is un-redacted:
   * secrets were redacted by the daemon before they reached the stream.
   *
   * The subscription, `schedule` and `settings` arrive as props so a test can drive the component;
   * the route owns the `EventSource`.
   *
   * ── Batching ────────────────────────────────────────────────────────────────────────────────────
   *
   * At trace level the stream runs at hundreds of events per second. Assigning `$state` per event
   * reconciles a 1000-key `{#each}` that often and freezes the tab, so arrivals accumulate into the
   * log and a single publish is scheduled through `schedule`, ten times a second
   * (`PUBLISH_INTERVAL_MS` in `$lib/inspector-view`). The tail-scroll `$effect` rides the same batch.
   *
   * `content-visibility: auto` on the rows lets the browser skip layout and paint for rows out of
   * view, so the full 1000-entry scrollback costs roughly what the visible dozen cost and no "show
   * more" control is needed. `contain-intrinsic-size` keeps the scrollbar accurate.
   *
   * `schedule` is a prop so tests can deliver frames synchronously and assert after `flushSync`
   * instead of waiting for a timer.
   *
   * ── The filter is applied by the daemon ─────────────────────────────────────────────────────────
   *
   * The verbosity and subsystem controls do not hide rows. They change the `FrameFilter` the
   * subscription is opened with, which becomes the `?level=`/`?source=` query, so excluded events are
   * never serialised. Narrowing is therefore a reconnect, and a reconnect starts a clean log so rows
   * from two filters are never mixed.
   *
   * The daemon assigns `seq` before filtering and does not renumber, so a filtered stream has holes
   * by construction; dropped-event reporting is switched off while a filter is on, and the UI says so.
   *
   * Because a filter change costs a reconnect, the choice is remembered
   * (`$lib/inspector-settings.svelte.ts`); under ingress every navigation is a full page load.
   *
   * ── Pausing is a view action ────────────────────────────────────────────────────────────────────
   *
   * Pause stops publishing, not receiving: arrivals keep accumulating and the held count is shown.
   * Closing the stream instead would lose the events that made you want to look. Pause is not
   * persisted, because an inspector that came back paused would look like one receiving nothing.
   *
   * ── Text the tests compare exactly ──────────────────────────────────────────────────────────────
   *
   * `FrameInspector.test.ts` compares `[data-testid="frame-count"]`'s `textContent` to `"5 events"`
   * and a gap row's text to a full sentence. Style them freely, but add no characters or text nodes
   * inside them.
   */
  import * as Select from '$lib/components/ui/select';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import { ArrowDown, ArrowDownLeft, ArrowUpRight, Eraser, History, Pause, Play } from '$lib/icons';
  import type { TraceFrame } from '$lib/api-types';
  import { FrameLog, type LogEntry } from '$lib/frame-log';
  import {
    InspectorSettings,
    inspectorSettings,
    saveInspectorSettings,
  } from '$lib/inspector-settings.svelte';
  import { Tail } from '$lib/inspector-tail';
  import {
    atPublishInterval,
    frameTime,
    keyOf,
    levelClass,
    levelLabel,
    type Schedule,
    SOURCE_HUES,
    SOURCE_LABELS,
  } from '$lib/inspector-view';
  import { type FrameFilter, isFiltered, TRACE_LEVELS, TRACE_SOURCES } from '$lib/sse';
  import { cn } from '$lib/utils.js';

  const {
    subscribe,
    schedule = atPublishInterval,
    settings = inspectorSettings,
  }: {
    subscribe: (onFrame: (frame: TraceFrame) => void, filter: FrameFilter) => () => void;
    schedule?: Schedule;
    settings?: InspectorSettings;
  } = $props();

  /**
   * Copied out of the reactive proxy, and narrowed on the way: what leaves this component is a plain
   * `FrameFilter` with anything unrecognised dropped, rather than a query the daemon cannot parse.
   */
  const filter = $derived<FrameFilter>(settings.filter);

  /**
   * The preference, written back whenever it changes.
   *
   * Reading `snapshot()` is what registers the dependencies, so this runs on a level, a source or a
   * follow change and at no other time. Persisting is best-effort, because a browser with site data
   * blocked must still get an inspector.
   */
  $effect(() => {
    saveInspectorSettings(settings.snapshot());
  });

  let log = new FrameLog();
  let entries = $state<readonly LogEntry[]>([]);
  let frameCount = $state(0);
  let gapsReported = $state(true);
  /** Publishing suspended by the operator. Receiving continues; see the header. */
  let paused = $state(false);
  /** How many frames have arrived since the screen was last redrawn. Only ever non-zero paused. */
  let held = $state(0);
  /**
   * The seq above which a row counts as having *just arrived*, and so gets the entrance.
   *
   * `Infinity` means "none of them", which is what the first publish uses: a new subscriber is handed
   * the daemon's whole retained ring, and a backlog is not an arrival to animate.
   *
   * The entrance cannot simply be "a row is new". With `content-visibility: auto`, a skipped row does
   * not run its animation until it is painted, so a row appended below the fold would play its
   * entrance whenever the operator later scrolled past it. Marking only the newest batch, and only
   * while the log is following, means the class is gone 100 ms later and a row that was never
   * painted never animates.
   */
  let freshAfter = $state(Number.POSITIVE_INFINITY);

  /**
   * The batching seam.
   *
   * `arrived` is set by the stream callback — which does nothing else — and cleared by `publish`;
   * `held` counts what has landed since the last redraw, which is what the paused badge reports.
   * `cancelPublish` holds the pending scheduler handle so a teardown does not leave a callback
   * pointing at a log that has been replaced.
   *
   * Held in a plain object rather than as `$state`: nothing reactive reads them, and making them
   * reactive would put a state write on the per-event path.
   */
  const batch: {
    arrived: boolean;
    held: number;
    newest: number;
    published: number;
    /** The last few `published` marks: rows newer than the oldest are still animating in. */
    recent: number[];
    cancelPublish: (() => void) | null;
  } = {
    arrived: false,
    held: 0,
    newest: 0,
    // What the previous redraw had already shown, so the next one knows which rows are its own.
    // `Infinity` until the first publish: see `freshAfter`.
    published: Number.POSITIVE_INFINITY,
    recent: [],
    cancelPublish: null,
  };

  /** How many redraws a fresh row keeps its entrance class: long enough for the animation. */
  const ENTRANCE_BATCHES = 3;

  /**
   * Moves everything accumulated since the last batch onto the screen, in one reassignment.
   *
   * Paused, it publishes the *count* and nothing else: one scalar write per 100 ms, no snapshot of
   * the ring and no reconciliation of a 1000-key `{#each}`, so pausing is genuinely cheaper.
   */
  function publish(): void {
    batch.arrived = false;
    batch.cancelPublish = null;
    if (paused) {
      held = batch.held;
      return;
    }
    batch.held = 0;
    held = 0;
    // Only a log that is following animates its arrivals; see `freshAfter`.
    batch.recent = [...batch.recent, batch.published].slice(-ENTRANCE_BATCHES);
    freshAfter = settings.follow ? Math.min(...batch.recent) : Number.POSITIVE_INFINITY;
    batch.published = batch.newest;
    entries = log.entries;
    frameCount = log.frameCount;
  }

  /** Publishes now, cancelling whatever was scheduled, so an operator action lands immediately. */
  function publishNow(): void {
    batch.cancelPublish?.();
    publish();
  }

  $effect(() => {
    // Reading `filter` is what makes a filter change reopen the stream.
    const current = filter;
    log = new FrameLog({ detectGaps: !isFiltered(current) });
    entries = log.entries;
    frameCount = 0;
    batch.held = 0;
    batch.published = Number.POSITIVE_INFINITY;
    batch.recent = [];
    held = 0;
    freshAfter = Number.POSITIVE_INFINITY;
    gapsReported = log.detectsGaps;

    // Read-only subscription: the inspector never sends anything to the daemon.
    //
    // The callback's whole job is one `append`, one increment and, at most once per batch, one
    // scheduled publish. Anything else here runs once per event.
    const unsubscribe = subscribe((received) => {
      log.append(received);
      batch.held += 1;
      batch.newest = received.seq;
      if (!batch.arrived) {
        batch.arrived = true;
        batch.cancelPublish = schedule(publish);
      }
    }, current);

    return () => {
      unsubscribe();
      batch.cancelPublish?.();
      batch.arrived = false;
      batch.cancelPublish = null;
    };
  });

  /**
   * A clean baseline, on an explicit operator action only.
   *
   * Clearing discards what the browser has accumulated and sends nothing to the daemon. The log is a
   * 1000-entry ring, so without this, diagnosing a second handshake attempt means reading it
   * interleaved with the first. It is also the only way to retire a stale gap entry.
   */
  function clearLog(): void {
    log.clear();
    batch.held = 0;
    batch.published = Number.POSITIVE_INFINITY;
    publishNow();
  }

  /** Suspends or resumes the redraw. Resuming shows the backlog at once rather than up to 100 ms late. */
  function togglePaused(): void {
    paused = !paused;
    publishNow();
  }

  /**
   * Tailing: the log follows its newest row until the operator scrolls away from it.
   *
   * `$lib/inspector-tail.ts` holds the whole of that decision, including why "are we at the bottom?"
   * is the wrong question to ask of a list whose rows are measured lazily.
   */
  const tail = new Tail(() => settings);

  $effect(() => {
    // `entries` is the dependency: a new row is what may need scrolling to. Reading `scrollHeight`
    // forces a layout, so this is deliberately driven by `entries` — which changes once per
    // published batch, never by the per-event arrival path.
    void entries.length;
    if (settings.follow) {
      tail.follow();
    }
  });
</script>

<section class="flex flex-col gap-4">
  <div class="glass flex flex-col gap-4 rounded-3xl border p-4">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <p class="flex flex-wrap items-center gap-2 text-sm">
        <span class="relative flex size-2.5" aria-hidden="true">
          {#if !paused}
            <span class="bg-success absolute inset-0 animate-ping rounded-full opacity-60"></span>
          {/if}
          <span class={cn('relative size-2.5 rounded-full', paused ? 'bg-warning' : 'bg-success')}
          ></span>
        </span>
        <span class="font-semibold">{paused ? 'Paused' : 'Live'}</span>
        <span class="text-muted-foreground tabular-nums" data-testid="frame-count"
          >{frameCount} events</span
        >
        {#if paused}
          <span
            class="bg-warning/15 text-warning rounded-full px-2 py-0.5 text-xs font-medium"
            data-testid="frame-held"
          >
            paused — {held} held
          </span>
        {:else if !settings.follow}
          <span class="text-muted-foreground text-xs" data-testid="frame-parked">not following</span
          >
        {/if}
      </p>
      <div class="flex items-center gap-2">
        <button
          type="button"
          data-testid="frame-pause"
          aria-pressed={paused}
          onclick={togglePaused}
          class={cn(
            'press inline-flex h-9 items-center gap-1.5 rounded-full px-4 text-sm font-medium',
            paused
              ? 'bg-primary text-primary-foreground'
              : 'bg-surface-strong hover:bg-card border',
          )}
        >
          {#if paused}
            <Play class="size-3.5" aria-hidden="true" />
            Resume
          {:else}
            <Pause class="size-3.5" aria-hidden="true" />
            Pause
          {/if}
        </button>
        <button
          type="button"
          data-testid="frame-clear"
          disabled={entries.length === 0}
          onclick={clearLog}
          title="Clear"
          aria-label="Clear"
          class="press bg-surface-strong hover:bg-card inline-flex size-9 items-center justify-center rounded-full border disabled:opacity-50"
        >
          <Eraser class="size-4" aria-hidden="true" />
        </button>
      </div>
    </div>

    <div class="flex flex-col gap-3 lg:flex-row lg:items-center">
      <Select.Root type="single" bind:value={settings.level}>
        <Select.Trigger
          data-testid="frame-level-filter"
          aria-label="Level"
          class="bg-surface-strong h-9 w-full rounded-full sm:w-52"
          >{levelLabel(settings.level)}</Select.Trigger
        >
        <Select.Content>
          <Select.Group>
            {#each TRACE_LEVELS as option (option)}
              <Select.Item value={option} label={levelLabel(option)}
                >{levelLabel(option)}</Select.Item
              >
            {/each}
          </Select.Group>
        </Select.Content>
      </Select.Root>

      <ToggleGroup.Root
        type="multiple"
        spacing={2}
        bind:value={settings.sources}
        class="flex flex-wrap justify-start gap-2"
        aria-label="Sources"
      >
        {#each TRACE_SOURCES as option (option)}
          <ToggleGroup.Item
            value={option}
            data-testid="frame-source-{option}"
            aria-label={SOURCE_LABELS[option]}
            style="--hue: {SOURCE_HUES[option]}; --chroma: {option === 'other' ? 0.02 : 0.16}"
            class="press bg-surface-strong data-[state=on]:border-[oklch(0.65_var(--chroma)_var(--hue))] data-[state=on]:bg-[oklch(0.65_var(--chroma)_var(--hue)/0.15)] h-9 shrink-0 gap-2 rounded-full border px-3.5 text-sm capitalize"
          >
            <span
              class="size-2 rounded-full bg-[oklch(0.65_var(--chroma)_var(--hue))]"
              aria-hidden="true"
            ></span>
            {SOURCE_LABELS[option]}
          </ToggleGroup.Item>
        {/each}
      </ToggleGroup.Root>
    </div>
  </div>

  {#if !gapsReported}
    <p
      class="bg-surface-strong text-muted-foreground rounded-2xl border px-4 py-2.5 text-xs"
      data-testid="frame-gaps-off"
    >
      Dropped events aren’t reported while filtering.
    </p>
  {/if}

  <div class="glass relative overflow-hidden rounded-3xl border">
    <div
      bind:this={tail.box}
      onscroll={() => tail.onScroll()}
      {@attach (box) => tail.watch(box)}
      role="log"
      aria-label="Trace frames"
      data-testid="frame-log"
      class="no-scrollbar scroll-fade h-[62dvh] min-h-64 scroll-smooth overflow-x-hidden overflow-y-auto overscroll-contain md:h-[70dvh]"
    >
      {#if entries.length === 0}
        <div
          class="text-muted-foreground flex h-full flex-col items-center justify-center gap-2 p-6 text-sm"
          data-testid="no-frames"
        >
          <span class="bg-primary/60 size-2 animate-ping rounded-full" aria-hidden="true"></span>
          Waiting for events…
        </div>
      {:else}
        <ol class="divide-border/50 divide-y" {@attach (list) => tail.track(list)}>
          {#each entries as entry (keyOf(entry))}
            {#if entry.row === 'gap'}
              <li
                class="bg-destructive/10 text-destructive px-4 py-2.5 text-xs font-medium"
                class:frame-row-in={entry.before > freshAfter}
                data-testid="frame-gap"
              >
                {entry.missing} event{entry.missing === 1 ? '' : 's'} dropped between seq
                {entry.after} and {entry.before}
              </li>
            {:else}
              {@const frame = entry.frame}
              {@const fields = Object.entries(frame.fields)}
              {@const direction = frame.fields.direction}
              <li
                class="frame-row relative"
                class:frame-row-in={frame.seq > freshAfter}
                style="--hue: {SOURCE_HUES[frame.source] ?? 250}; --chroma: {frame.source ===
                'other'
                  ? 0.02
                  : 0.16}"
                data-testid="frame-row"
                data-seq={frame.seq}
              >
                <span
                  class="absolute inset-y-2 left-0 w-[3px] rounded-r-full bg-[oklch(0.65_var(--chroma)_var(--hue))]"
                  aria-hidden="true"
                ></span>
                <details class="group">
                  <summary
                    class={cn(
                      'hover:bg-foreground/[0.03] flex list-none flex-col gap-1 py-2.5 pr-4 pl-5 [&::-webkit-details-marker]:hidden',
                      fields.length > 0 ? 'cursor-pointer' : 'cursor-default',
                    )}
                  >
                    <span class="flex min-w-0 items-center gap-2 text-xs">
                      <time
                        class="text-foreground/80 shrink-0 font-mono tabular-nums"
                        datetime={new Date(frame.timestamp_ms).toISOString()}
                        title="#{frame.seq}"
                        data-testid="frame-time">{frameTime(frame.timestamp_ms)}</time
                      >
                      <span
                        class="text-muted-foreground/60 hidden shrink-0 font-mono text-[0.65rem] tabular-nums sm:inline"
                        data-testid="frame-seq">#{frame.seq}</span
                      >
                      <span
                        class={cn(
                          'shrink-0 rounded-full border px-1.5 py-px text-[0.65rem] font-semibold uppercase',
                          levelClass(frame.level),
                        )}
                        data-testid="frame-level">{frame.level}</span
                      >
                      <span
                        class="shrink-0 font-medium text-[oklch(0.6_var(--chroma)_var(--hue))] capitalize"
                        data-testid="frame-row-source"
                        >{SOURCE_LABELS[frame.source] ?? frame.source}</span
                      >
                      {#if direction !== undefined}
                        <span
                          class="text-muted-foreground inline-flex shrink-0 items-center"
                          title={direction}
                        >
                          {#if direction
                            .toLowerCase()
                            .startsWith('out') || direction.toLowerCase() === 'tx'}
                            <ArrowUpRight class="size-3.5" aria-hidden="true" />
                          {:else}
                            <ArrowDownLeft class="size-3.5" aria-hidden="true" />
                          {/if}
                        </span>
                      {/if}
                      <span
                        class="text-muted-foreground/70 min-w-0 truncate font-mono"
                        data-testid="frame-target">{frame.target}</span
                      >
                      <span class="ml-auto flex min-w-0 items-center gap-1.5">
                        <span
                          class="bg-muted text-muted-foreground truncate rounded-full px-1.5 py-px text-[0.65rem]"
                          data-testid="frame-kind">{frame.kind}</span
                        >
                        {#if frame.replayed}
                          <span
                            class="text-muted-foreground/70 inline-flex shrink-0 items-center gap-1 rounded-full border border-dashed px-1 py-px text-[0.65rem] sm:px-1.5"
                            title="Replayed from the daemon's buffer"
                            data-testid="frame-replayed"
                            ><History class="size-3" aria-hidden="true" /><span
                              class="sr-only sm:not-sr-only">replayed</span
                            ></span
                          >
                        {/if}
                      </span>
                    </span>
                    <span
                      class="line-clamp-2 text-sm break-words group-open:line-clamp-none"
                      data-testid="frame-message">{frame.message}</span
                    >
                    {#if fields.length > 0}
                      <span class="text-muted-foreground text-[0.7rem] group-open:hidden"
                        >{fields.length} field{fields.length === 1 ? '' : 's'}</span
                      >
                    {/if}
                  </summary>
                  {#if fields.length > 0}
                    <dl
                      class="bg-muted/40 mx-4 mb-3 ml-5 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 rounded-xl p-3 text-xs"
                      data-testid="frame-fields"
                    >
                      {#each fields as [key, value] (key)}
                        <dt class="text-muted-foreground font-mono">{key}</dt>
                        <dd class="font-mono break-all">{value}</dd>
                      {/each}
                    </dl>
                  {/if}
                </details>
              </li>
            {/if}
          {/each}
        </ol>
      {/if}
    </div>

    {#if !settings.follow && entries.length > 0}
      <button
        type="button"
        data-testid="frame-jump"
        onclick={() => tail.jump()}
        class="press bg-primary text-primary-foreground absolute bottom-4 left-1/2 inline-flex h-9 -translate-x-1/2 items-center gap-1.5 rounded-full px-4 text-sm font-medium shadow-[0_8px_24px_-8px_var(--primary)]"
      >
        <ArrowDown class="size-4" aria-hidden="true" />
        Jump to latest
      </button>
    {/if}
  </div>
</section>
