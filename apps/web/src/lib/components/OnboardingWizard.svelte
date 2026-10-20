<script lang="ts">
  /**
   * The onboarding wizard, in two phases.
   *
   * 1. **Read the profile.** `preview` parses the archive and reports what it contains. Nothing is
   *    dialled, registered or stored.
   * 2. **Confirm.** A dialog shows what was parsed and collects what the archive cannot know: what to
   *    call the device, where it is, and on which port. Prefilled and editable.
   * 3. **Commit.** `commit` re-posts the same ZIP with those choices, and the daemon registers,
   *    stores, dials and reports.
   *
   * The API calls arrive as props so the component is testable without a daemon and
   * `AddApplianceSheet` stays the one place that knows about the credential. Nothing is sent on
   * mount: phase 1 needs a click, and phase 2 a second one.
   *
   * The host field starts empty: the profile-derived host contains the `haId`, so the daemon does
   * not return it (see `ProfilePreview`), and it would not resolve anyway.
   *
   * Committing stores the device whether or not it answered, so a wrong address is correctable. The
   * address form below the result is that correction, and it is offered on success too: it is a
   * normal input, not the error path.
   */
  import { Ban, CircleCheck, FileArchive, TriangleAlert } from '$lib/icons';
  import type { CommitRequest, OnboardingResult, ProfilePreview } from '$lib/api-types';
  import * as Dialog from '$lib/components/ui/dialog';
  import { cn } from '$lib/utils.js';

  const {
    preview,
    commit,
    setManualHost,
  }: {
    preview: (zip: File) => Promise<ProfilePreview>;
    commit: (zip: File, request: CommitRequest) => Promise<OnboardingResult>;
    setManualHost: (slug: string, host: string) => Promise<OnboardingResult>;
  } = $props();

  let chosen = $state<File | null>(null);
  let parsed = $state<ProfilePreview | null>(null);
  let confirming = $state(false);
  let result = $state<OnboardingResult | null>(null);
  let failure = $state<string | null>(null);
  let busy = $state(false);

  /** The dialog's three fields. Strings, because that is what an `<input>` holds. */
  let slug = $state('');
  let host = $state('');
  let port = $state('');

  function chooseFile(event: Event): void {
    const input = event.currentTarget;
    chosen = input instanceof HTMLInputElement ? (input.files?.[0] ?? null) : null;
  }

  /**
   * Phase 1. Parses the archive and opens the confirmation dialog with its fields prefilled.
   *
   * `port` is prefilled with the transport's default so the operator can see what they would be
   * overriding; `host` is not prefilled at all (see the module comment).
   */
  async function read(): Promise<void> {
    if (chosen === null || busy) {
      return;
    }
    busy = true;
    failure = null;
    try {
      const report = await preview(chosen);
      parsed = report;
      slug = report.suggested_slug;
      host = '';
      port = String(report.default_port);
      confirming = true;
    } catch (error) {
      // The daemon's `ApiError.message` is operator-facing prose; it never holds the token or a stack.
      failure = error instanceof Error ? error.message : 'the profile could not be read';
    } finally {
      busy = false;
    }
  }

  /**
   * Phase 2. Sends the operator's confirmed choices with the same archive.
   *
   * The port is sent as `null` when it still equals the transport's default, so the daemon stores "no
   * override" rather than a pinned number: a default that changes later should follow the transport,
   * not a value the operator never chose.
   *
   * The host is sent verbatim: no trimming, lower-casing or IPv6 bracketing. The daemon decides what
   * counts as a host, and a well-meant client-side "fix" is how a working address becomes a broken one.
   * `trim()` appears only in the emptiness guard.
   */
  async function confirm(): Promise<void> {
    if (chosen === null || parsed === null || busy || !canConfirm) {
      return;
    }
    const chosenPort = Number(port);
    busy = true;
    failure = null;
    try {
      result = await commit(chosen, {
        slug,
        host: host.trim() === '' ? null : host,
        // `Number('')` is 0 rather than NaN, so emptiness is tested on the string. Both mean "no
        // override", and so does a value still equal to the prefilled default.
        port:
          port.trim() === '' || Number.isNaN(chosenPort) || chosenPort === parsed.default_port
            ? null
            : chosenPort,
      });
      confirming = false;
    } catch (error) {
      // The dialog stays open on failure: a rejected slug or an occupied name is fixed *here*, and
      // closing would make the operator start from the file picker again.
      failure = error instanceof Error ? error.message : 'the appliance could not be added';
    } finally {
      busy = false;
    }
  }

  /**
   * The two client-side rules, and there are deliberately only two.
   *
   * A name must be non-empty and a port, if typed, must be a number in range. Everything else — which
   * characters a slug may contain, whether a name is taken, whether a host is reachable — is the
   * daemon's judgement, and it answers with prose the dialog renders.
   */
  const portIsUsable = $derived.by(() => {
    // An absent port is valid: it means the transport's default.
    if (port.trim() === '') {
      return true;
    }
    const value = Number(port);
    return Number.isInteger(value) && value > 0 && value <= 65_535;
  });
  const canConfirm = $derived(slug.trim() !== '' && portIsUsable);

  /** Whether the daemon says this profile would update a device already onboarded. */
  const updating = $derived(parsed?.existing_slug ?? null);

  let manualHost = $state('');

  /**
   * Four explicit handlers rather than `bind:value`, and rather than one handler factory.
   *
   * Not `bind:`: the variable is never textually reassigned, and oxlint's `prefer-const` cannot see
   * the compiler's rewrite. Not a factory taking a setter either, which hides the assignment inside a
   * closure where `prefer-const` cannot see it either.
   */
  function typed(event: Event): string | null {
    const input = event.currentTarget;
    return input instanceof HTMLInputElement ? input.value : null;
  }

  function editSlug(event: Event): void {
    slug = typed(event) ?? slug;
  }

  function editHost(event: Event): void {
    host = typed(event) ?? host;
  }

  function editPort(event: Event): void {
    port = typed(event) ?? port;
  }

  function editManualHost(event: Event): void {
    manualHost = typed(event) ?? manualHost;
  }

  /**
   * `needs_manual_ip` is emphasis, not a gate. The form below the result is always offered;
   * this decides whether it is presented as the thing to do next.
   */
  const needsManualIp = $derived(
    result !== null && result.connection.status === 'failed' && result.connection.needs_manual_ip,
  );

  /** One click, one attempt. The UI never retries by itself. */
  async function retryAtManualHost(): Promise<void> {
    if (result === null || manualHost.trim() === '' || busy) {
      return;
    }
    busy = true;
    failure = null;
    try {
      result = await setManualHost(result.device.slug, manualHost);
    } catch (error) {
      failure = error instanceof Error ? error.message : 'the connection test failed';
    } finally {
      busy = false;
    }
  }

  const FIELD =
    'border-input bg-background focus-visible:border-ring focus-visible:ring-ring/50 h-9 w-full rounded-lg border px-2.5 text-sm transition-colors focus-visible:ring-[3px] focus-visible:outline-none';
  const BUTTON =
    'press bg-primary text-primary-foreground hover:bg-primary/90 inline-flex h-9 items-center rounded-lg px-3 text-sm font-medium disabled:opacity-50';
</script>

<section class="flex max-w-2xl flex-col gap-5">
  <div class="bg-card flex flex-col gap-3 rounded-xl border p-4">
    <div class="flex items-baseline gap-2">
      <span
        class="bg-muted text-muted-foreground flex size-5 shrink-0 items-center justify-center rounded-full text-[0.7rem] font-semibold"
        aria-hidden="true">1</span
      >
      <h2 class="text-sm font-semibold tracking-tight">The profile ZIP</h2>
    </div>

    <!--
      A real `<input type="file">`, stretched invisibly across the label: it keeps the keyboard and the
      mobile file picker working, makes the whole label a drop target, and lets the test drive it with
      `attachFile`, without rendering the browser's own file widget.
    -->
    <label
      class="press border-input hover:border-ring/60 hover:bg-muted/40 focus-within:border-ring focus-within:ring-ring/50 relative flex cursor-pointer flex-col items-center gap-1.5 rounded-xl border border-dashed p-6 text-center focus-within:ring-[3px]"
    >
      <FileArchive class="text-muted-foreground size-5" aria-hidden="true" />
      {#if chosen === null}
        <span class="text-sm font-medium">Choose a profile ZIP</span>
        <span class="text-muted-foreground text-xs">or drop it here</span>
      {:else}
        <span class="max-w-full truncate font-mono text-sm font-medium" data-testid="profile-chosen"
          >{chosen.name}</span
        >
        <span class="text-muted-foreground text-xs">
          {(chosen.size / 1024).toFixed(0)} kB
        </span>
      {/if}
      <input
        data-testid="profile-zip"
        type="file"
        accept=".zip,application/zip"
        onchange={chooseFile}
        aria-label="Profile ZIP"
        class="absolute inset-0 cursor-pointer opacity-0"
      />
    </label>

    <div>
      <button
        data-testid="upload"
        type="button"
        disabled={chosen === null || busy}
        onclick={read}
        class={BUTTON}
      >
        {busy && !confirming ? 'Reading…' : 'Read profile'}
      </button>
    </div>

    {#if failure !== null && !confirming}
      <p
        class="border-destructive/40 bg-destructive/5 text-destructive flex items-start gap-2 rounded-lg border p-2.5 text-sm"
        data-testid="upload-error"
      >
        <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <span>{failure}</span>
      </p>
    {/if}
  </div>

  <!-- Phase 2. `bind:open` so Escape and the overlay close it; cancelling discards nothing but the
       dialog's own fields, and the chosen file stays picked so a re-read is one click. -->
  <Dialog.Root bind:open={confirming}>
    <Dialog.Content class="sm:max-w-lg" data-testid="confirm-dialog">
      <Dialog.Header>
        <Dialog.Title>
          {updating === null ? 'Add this appliance' : 'Update this appliance'}
        </Dialog.Title>
        <Dialog.Description>
          {#if updating === null}
            Check it, then set the address.
          {:else}
            Replaces the profile of <span class="font-mono">{updating}</span>.
          {/if}
        </Dialog.Description>
      </Dialog.Header>

      {#if parsed !== null}
        <dl
          class="bg-muted/40 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 rounded-lg p-3 text-sm"
        >
          <dt class="text-muted-foreground text-xs">Appliance</dt>
          <dd class="font-mono text-xs" data-testid="preview-role">{parsed.role}</dd>
          <dt class="text-muted-foreground text-xs">Features</dt>
          <dd data-testid="preview-entity-count">{parsed.entity_count}</dd>
          <dt class="text-muted-foreground text-xs">Skipped</dt>
          <dd data-testid="preview-skipped">{parsed.skipped_entity_count}</dd>
          <dt class="text-muted-foreground text-xs">Connection</dt>
          <dd class="font-mono text-xs" data-testid="preview-transport">
            {parsed.transport === 'aes' ? 'AES over ws' : 'TLS-PSK over wss'}
          </dd>
        </dl>

        <div class="flex flex-col gap-3">
          <label class="flex flex-col gap-1">
            <span class="text-muted-foreground text-xs font-medium">Name</span>
            <input
              data-testid="confirm-slug"
              type="text"
              autocomplete="off"
              spellcheck="false"
              value={slug}
              oninput={editSlug}
              class={cn(FIELD, 'font-mono')}
            />
            <span class="text-muted-foreground text-xs"> Used in MQTT topics. </span>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-muted-foreground text-xs font-medium">Address</span>
            <input
              data-testid="confirm-host"
              type="text"
              inputmode="url"
              autocomplete="off"
              spellcheck="false"
              placeholder="10.0.0.42"
              value={host}
              oninput={editHost}
              class={cn(FIELD, 'font-mono')}
            />
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-muted-foreground text-xs font-medium">Port</span>
            <input
              data-testid="confirm-port"
              type="text"
              inputmode="numeric"
              autocomplete="off"
              value={port}
              oninput={editPort}
              class={cn(FIELD, 'font-mono', !portIsUsable && 'border-destructive')}
              aria-invalid={!portIsUsable}
            />
          </label>
        </div>
      {/if}

      {#if failure !== null}
        <p
          class="border-destructive/40 bg-destructive/5 text-destructive flex items-start gap-2 rounded-lg border p-2.5 text-sm"
          data-testid="confirm-error"
        >
          <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>{failure}</span>
        </p>
      {/if}

      <Dialog.Footer>
        <button
          data-testid="confirm-cancel"
          type="button"
          disabled={busy}
          onclick={() => (confirming = false)}
          class="press hover:bg-muted inline-flex h-9 items-center rounded-lg border px-3 text-sm font-medium disabled:opacity-50"
        >
          Cancel
        </button>
        <button
          data-testid="confirm-submit"
          type="button"
          disabled={busy || !canConfirm}
          onclick={confirm}
          class={BUTTON}
        >
          {#if busy}
            Connecting…
          {:else if updating === null}
            Add appliance
          {:else}
            Update appliance
          {/if}
        </button>
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>

  {#if result !== null}
    {@const connected = result.connection.status !== 'failed'}
    <div class="bg-card flex flex-col gap-3 rounded-xl border p-4">
      <div class="flex items-baseline gap-2">
        <span
          class="bg-muted text-muted-foreground flex size-5 shrink-0 items-center justify-center rounded-full text-[0.7rem] font-semibold"
          aria-hidden="true">2</span
        >
        <h2 class="text-sm font-semibold tracking-tight">What Kessel added</h2>
      </div>

      <dl class="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-sm">
        <dt class="text-muted-foreground text-xs">Device</dt>
        <dd class="font-mono text-xs break-all" data-testid="result-slug">{result.device.slug}</dd>
        <dt class="text-muted-foreground text-xs">Role</dt>
        <dd class="font-mono text-xs" data-testid="result-role">{result.device.role}</dd>
        <dt class="text-muted-foreground text-xs">Features</dt>
        <dd data-testid="result-entity-count">{result.device.entity_count}</dd>
        <dt class="text-muted-foreground text-xs">Skipped entities</dt>
        <dd data-testid="result-skipped">{result.skipped_entity_count}</dd>
        <dt class="text-muted-foreground text-xs">Host</dt>
        <dd class="font-mono text-xs" data-testid="result-host-source">{result.host_source}</dd>
        <dt class="text-muted-foreground text-xs">Connection</dt>
        <dd
          class={cn('flex items-start gap-1.5', !connected && 'text-destructive')}
          data-testid="result-connection"
        >
          {#if connected}
            <CircleCheck class="mt-0.5 size-3.5 shrink-0 text-success" aria-hidden="true" />
          {:else}
            <Ban class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          {/if}
          <span>
            {result.connection.status}
            {#if result.connection.status === 'failed'}— {result.connection.reason}{/if}
          </span>
        </dd>
      </dl>

      {#if !connected}
        <p class="text-muted-foreground text-xs" data-testid="saved-anyway">
          Saved anyway. Fix the address below.
        </p>
      {/if}
    </div>

    <!--
      `class:emphasised` carries no styling; the Tailwind classes beside it do that. It is the
      behavioural marker `OnboardingWizard.test.ts` asserts in both directions, so the test does not
      depend on utility classes. Keep it.
    -->
    <div
      class={cn(
        'flex flex-col gap-3 rounded-xl border p-4',
        needsManualIp ? 'border-destructive/50 bg-destructive/5' : 'bg-card',
      )}
      class:emphasised={needsManualIp}
      data-testid="manual-host-prompt"
    >
      <div class="flex items-baseline gap-2">
        <span
          class="bg-muted text-muted-foreground flex size-5 shrink-0 items-center justify-center rounded-full text-[0.7rem] font-semibold"
          aria-hidden="true">3</span
        >
        <h2 class="text-sm font-semibold tracking-tight">Correct the address</h2>
      </div>

      <p
        class={cn(
          'text-sm',
          needsManualIp ? 'text-destructive font-medium' : 'text-muted-foreground',
        )}
        data-testid="manual-host-reason"
      >
        {#if needsManualIp}
          Not reachable at that address.
        {:else}
          Connected.
        {/if}
      </p>

      <label class="flex flex-col gap-1">
        <span class="text-muted-foreground text-xs font-medium">Appliance host or IP</span>
        <input
          data-testid="manual-host"
          type="text"
          inputmode="url"
          autocomplete="off"
          spellcheck="false"
          placeholder="10.0.0.42"
          value={manualHost}
          oninput={editManualHost}
          class={cn(FIELD, 'font-mono')}
        />
      </label>

      <div>
        <button
          data-testid="manual-host-submit"
          type="button"
          disabled={manualHost.trim() === '' || busy}
          onclick={retryAtManualHost}
          class={BUTTON}
        >
          {busy ? 'Connecting…' : 'Try this host'}
        </button>
      </div>
    </div>
  {/if}
</section>
