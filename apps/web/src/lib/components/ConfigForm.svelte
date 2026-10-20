<script lang="ts">
  /**
   * The settings form, chiefly how the operator points Kessel at their MQTT broker, which is how
   * Home Assistant sees the appliances.
   *
   * The inputs are uncontrolled: initialised from the prop in the markup and read back with
   * `FormData` on submit. Fields re-derived from a prop would overwrite what the operator is typing
   * whenever anything upstream re-rendered. That is also why the markup is a single `<form>` with
   * `name`d inputs: `new FormData(form)` only works if the inputs are really in the form.
   *
   * Deliberate behaviours:
   *
   * 1. The password field starts empty and is not sent when empty. The daemon never returns the
   *    stored password, so sending an empty value on every save would wipe it; omitting it means
   *    "keep". Clearing is a separate button so it cannot happen by accident.
   * 2. No client-side validation of the host: the daemon owns that judgement, and a well-meant
   *    client-side "fix" is how a working address becomes a broken one.
   * 3. It says a restart is required, because nothing here is applied live.
   */
  import * as Popover from '$lib/components/ui/popover';
  import * as Select from '$lib/components/ui/select';
  import { Switch } from '$lib/components/ui/switch';
  import { CircleCheck, CircleHelp, Info, TriangleAlert } from '$lib/icons';
  import RestartControl from './RestartControl.svelte';
  import type { ConfigUpdate, ConfigView, RestartOutcome } from '$lib/api-types';

  const {
    view,
    save,
    restart,
    health,
  }: {
    view: ConfigView;
    save: (update: ConfigUpdate) => Promise<ConfigView>;
    /** `POST /api/restart`: nothing here is applied live, so the page offers a restart. */
    restart: () => Promise<RestartOutcome>;
    /** `GET /api/health`, which {@link RestartControl} polls to see the daemon go and come back. */
    health: () => Promise<unknown>;
  } = $props();

  /** What the daemon last told us: the prop until a save replaces it. */
  let stored: ConfigView | null = $state(null);
  const current = $derived(stored ?? view);

  const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error'] as const;

  const DISCOVERY_MODES = [
    { value: 'full', label: 'full — every feature' },
    { value: 'curated', label: 'curated — a tidier set' },
  ] as const;

  /**
   * The two `Select`s need their current value in script, not just in the markup, because the trigger
   * renders the chosen option's *label* — a native `<select>` derived that from the DOM for free.
   *
   * Kept in sync from `current` by an effect rather than seeded once with `$state(view.…)`, which
   * would capture only the initial value. `current` changes exactly when a save returns, which is
   * when the stored value should win, the same rule every `value={current.…}` input follows.
   */
  let discovery = $state('full');
  let logLevel = $state('info');

  $effect(() => {
    discovery = current.mqtt_discovery ?? 'full';
    logLevel = current.log_level ?? 'info';
  });

  const discoveryLabel = $derived(
    DISCOVERY_MODES.find((mode) => mode.value === discovery)?.label ?? discovery,
  );

  let saving = $state(false);
  let justSaved = $state(false);
  let failure: string | null = $state(null);

  /** A trimmed field, or `null` when it is empty — which is how the daemon is told to unset it. */
  function field(data: FormData, name: string): string | null {
    const raw = data.get(name);
    const value = typeof raw === 'string' ? raw.trim() : '';
    return value === '' ? null : value;
  }

  async function submit(form: HTMLFormElement, clearPassword: boolean) {
    const data = new FormData(form);
    const typedPassword = data.get('mqtt_password');
    const password = typeof typedPassword === 'string' ? typedPassword : '';
    const port = field(data, 'mqtt_port');
    saving = true;
    justSaved = false;
    failure = null;
    try {
      stored = await save({
        // Sent verbatim: the daemon decides what a valid host is.
        mqtt_host: field(data, 'mqtt_host'),
        mqtt_port: port === null ? null : Number(port),
        mqtt_username: field(data, 'mqtt_username'),
        // '' clears, a value sets, and null (an empty box) keeps what is stored.
        mqtt_password: clearPassword ? '' : password === '' ? null : password,
        mqtt_base: field(data, 'mqtt_base'),
        mqtt_discovery: field(data, 'mqtt_discovery'),
        log_level: field(data, 'log_level'),
        // A checkbox is absent from `FormData` when it is unchecked, so this is the one field read as
        // a presence rather than a value. It is always sent: the daemon's "absent = keep" rule is for
        // clients that do not know the field.
        require_auth: data.get('require_auth') !== null,
      });
      form.reset();
      // The sign-in `Switch` takes `checked` from `current`, which the line above has just refreshed,
      // so no fix-up is needed after `reset()`.
      justSaved = true;
    } catch (error) {
      failure = error instanceof Error ? error.message : 'the settings could not be saved';
    } finally {
      saving = false;
    }
  }

  /**
   * What to say about the broker the daemon is actually using.
   *
   * On a fresh add-on install nothing is stored, so every box here is empty while the daemon is
   * publishing to the broker the Supervisor handed it. An operator reading an empty host box would
   * conclude MQTT is off and type a host, and because a typed host wins over autodiscovery, that
   * guess would replace a working broker.
   */
  const brokerNote = $derived.by(() => {
    const live = current.mqtt;
    const where =
      live.host === null ? '' : ` at ${live.host}${live.port === null ? '' : `:${live.port}`}`;
    switch (live.source) {
      case 'supervisor':
        return `Home Assistant's broker${where}, found automatically.`;
      case 'environment':
        return `Broker${where}, set by the container environment.`;
      case 'stored':
        return `Broker${where}.`;
      default:
        return live.autodiscovery
          ? 'No broker. Install the Mosquitto add-on, or enter a host.'
          : 'No broker. Enter a host, or turn on the add-on’s mqtt_autodiscovery option.';
    }
  });

  const FIELD =
    'border-input bg-background focus-visible:border-ring focus-visible:ring-ring/50 h-9 w-full rounded-lg border px-2.5 text-sm transition-colors focus-visible:ring-[3px] focus-visible:outline-none';
  const LABEL = 'text-muted-foreground text-xs font-medium';
</script>

<form
  class="flex max-w-2xl flex-col gap-5"
  data-testid="config-form"
  onsubmit={(event) => {
    event.preventDefault();
    void submit(event.currentTarget, false);
  }}
>
  <fieldset class="bg-card flex flex-col gap-3 rounded-xl border p-4">
    <legend class="sr-only">MQTT broker</legend>
    <h2 class="text-sm font-semibold tracking-tight">MQTT broker</h2>

    <p
      class="text-muted-foreground flex items-start gap-2 rounded-lg border border-dashed p-2.5 text-xs"
      data-testid="config-broker-source"
    >
      <Info class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
      <span>{brokerNote}</span>
    </p>

    <div class="grid gap-3 sm:grid-cols-[2fr_1fr]">
      <label class="flex flex-col gap-1">
        <span class={LABEL}>Host or IP</span>
        <input
          data-testid="config-host"
          name="mqtt_host"
          type="text"
          autocomplete="off"
          spellcheck="false"
          value={current.mqtt_host ?? ''}
          class="{FIELD} font-mono"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class={LABEL}>Port</span>
        <input
          data-testid="config-port"
          name="mqtt_port"
          type="text"
          autocomplete="off"
          value={current.mqtt_port === null ? '' : String(current.mqtt_port)}
          class="{FIELD} font-mono"
        />
      </label>
    </div>

    <div class="grid gap-3 sm:grid-cols-2">
      <label class="flex flex-col gap-1">
        <span class={LABEL}>Username</span>
        <input
          data-testid="config-username"
          name="mqtt_username"
          type="text"
          autocomplete="off"
          value={current.mqtt_username ?? ''}
          class={FIELD}
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class={LABEL}>Password</span>
        <input
          data-testid="config-password"
          name="mqtt_password"
          type="password"
          autocomplete="new-password"
          placeholder={current.mqtt_password_set ? 'Unchanged' : ''}
          class={FIELD}
        />
      </label>
    </div>

    <div class="grid gap-3 sm:grid-cols-2">
      <label class="flex flex-col gap-1">
        <span class={LABEL}>Topic root</span>
        <input
          data-testid="config-base"
          name="mqtt_base"
          type="text"
          autocomplete="off"
          spellcheck="false"
          value={current.mqtt_base ?? ''}
          class="{FIELD} font-mono"
        />
      </label>
      <!-- `name` is what makes bits-ui emit the hidden input the `FormData` read path needs. -->
      <div class="flex flex-col gap-1">
        <span class={LABEL} id="config-discovery-label">Discovery</span>
        <Select.Root type="single" name="mqtt_discovery" bind:value={discovery}>
          <Select.Trigger
            data-testid="config-discovery"
            aria-labelledby="config-discovery-label"
            class="h-9 w-full">{discoveryLabel}</Select.Trigger
          >
          <Select.Content>
            <Select.Group>
              {#each DISCOVERY_MODES as mode (mode.value)}
                <Select.Item value={mode.value} label={mode.label}>{mode.label}</Select.Item>
              {/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
      </div>
    </div>
  </fieldset>

  <fieldset class="bg-card flex flex-col gap-3 rounded-xl border p-4">
    <legend class="sr-only">Access</legend>
    <h2 class="text-sm font-semibold tracking-tight">Access</h2>
    <!--
      `name` makes bits-ui emit a hidden input, so the `FormData` read path above
      (`data.get('require_auth') !== null`) still covers this field.

      `checked={current.require_auth}` is deliberately not `bind:`: the switch owns its state while
      the operator is using it, and `current` only changes when a save returns, which is when the
      stored value should win.
    -->
    <!--
      The explanation is behind a `?` popover rather than printed underneath, so the prose does not
      drown the control. It still matters: it is the difference between "Home Assistant signs you in"
      and "you now need an API token from a file on the host".
    -->
    <div class="flex items-center gap-3">
      <Switch
        id="config-require-auth"
        data-testid="config-require-auth"
        name="require_auth"
        checked={current.require_auth}
      />
      <label for="config-require-auth" class="text-sm font-medium">Require sign-in for writes</label
      >
      <Popover.Root>
        <Popover.Trigger
          data-testid="config-require-auth-help"
          aria-label="What requiring sign-in does"
          class="press text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 rounded-full focus-visible:ring-[3px] focus-visible:outline-none"
        >
          <CircleHelp class="size-4" aria-hidden="true" />
        </Popover.Trigger>
        <Popover.Content align="start" class="max-w-xs text-xs leading-relaxed">
          For when something other than Home Assistant can reach Kessel. Writes then need the API
          token Kessel writes to a file on the host.
        </Popover.Content>
      </Popover.Root>
    </div>
  </fieldset>

  <fieldset class="bg-card flex flex-col gap-3 rounded-xl border p-4">
    <legend class="sr-only">Server</legend>
    <h2 class="text-sm font-semibold tracking-tight">Server</h2>
    <RestartControl {restart} {health} />
  </fieldset>

  <fieldset class="bg-card flex flex-col gap-3 rounded-xl border p-4">
    <legend class="sr-only">Logging</legend>
    <h2 class="text-sm font-semibold tracking-tight">Logging</h2>
    <div class="flex max-w-xs flex-col gap-1">
      <span class={LABEL} id="config-log-level-label">Log level</span>
      <Select.Root type="single" name="log_level" bind:value={logLevel}>
        <Select.Trigger
          data-testid="config-log-level"
          aria-labelledby="config-log-level-label"
          class="h-9 w-full">{logLevel}</Select.Trigger
        >
        <Select.Content>
          <Select.Group>
            {#each LOG_LEVELS as level (level)}
              <Select.Item value={level} label={level}>{level}</Select.Item>
            {/each}
          </Select.Group>
        </Select.Content>
      </Select.Root>
    </div>
  </fieldset>

  <div class="flex flex-wrap gap-2">
    <button
      data-testid="config-save"
      type="submit"
      disabled={saving}
      class="press bg-primary text-primary-foreground hover:bg-primary/90 inline-flex h-9 items-center rounded-lg px-3 text-sm font-medium disabled:opacity-50"
    >
      {saving ? 'Saving…' : 'Save settings'}
    </button>
    {#if current.mqtt_password_set}
      <button
        data-testid="config-clear-password"
        type="button"
        disabled={saving}
        onclick={(event) => {
          const form = (event.currentTarget as HTMLButtonElement).form;
          if (form) void submit(form, true);
        }}
        class="press text-destructive hover:bg-destructive/10 inline-flex h-9 items-center rounded-lg border px-3 text-sm font-medium disabled:opacity-50"
      >
        Clear the stored password
      </button>
    {/if}
  </div>

  {#if justSaved}
    <div
      class="border-success/40 bg-success/5 flex flex-col gap-3 rounded-xl border p-3 text-sm"
      data-testid="config-saved"
    >
      <p class="flex items-start gap-2">
        <CircleCheck class="text-success mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <span>Saved. Restart to apply.</span>
      </p>
      <!-- The button beside the message that says it is needed. The Server section below keeps its
           own, because a restart is also worth having when nothing was just saved. -->
      <RestartControl {restart} {health} label="Restart now" />
    </div>
  {/if}
  {#if failure}
    <p
      class="border-destructive/40 bg-destructive/5 text-destructive flex items-start gap-2 rounded-xl border p-3 text-sm"
      data-testid="config-error"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <span>{failure}</span>
    </p>
  {/if}

  {#if current.overridden_by_env.length > 0}
    <p
      class="text-muted-foreground flex items-start gap-2 rounded-xl border border-dashed p-3 text-xs"
      data-testid="config-env-override"
    >
      <Info class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
      <span>
        Overridden by the container:
        <span class="font-mono">{current.overridden_by_env.join(', ')}</span>
      </span>
    </p>
  {/if}
</form>
