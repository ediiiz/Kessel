<script lang="ts">
  /**
   * The restart button.
   *
   * Every setting on the settings page needs a restart (nothing in the daemon is hot-reloaded).
   * `POST /api/restart` asks the Supervisor to do it, so the button belongs beside the message that
   * says it is needed.
   *
   * Deliberate behaviours:
   *
   * 1. **Two clicks, no modal.** A restart drops every appliance connection, and under ingress the
   *    page goes with it, which is worth one confirmation. The confirmation is the button changing
   *    what it says, which cannot be click-through-dismissed the way a modal can.
   * 2. **The daemon's own sentence is shown.** `supervisor` and `process_exit` promise different
   *    things (only the first says Kessel comes back), and that judgement is made once, in the daemon.
   * 3. **It watches for the daemon to come back**, because the 202 is a promise, not a fact. A
   *    Supervisor that refuses (no `hassio_api: true`) produces the same 202.
   */
  import { CircleCheck, LoaderCircle, Power, TriangleAlert } from '$lib/icons';
  import type { RestartOutcome } from '$lib/api-types';

  const {
    restart,
    health,
    label = 'Restart Kessel',
    pollMs = 1000,
    timeoutMs = 60_000,
  }: {
    restart: () => Promise<RestartOutcome>;
    /** Any read that fails while the daemon is down — the page passes `GET /api/health`. */
    health: () => Promise<unknown>;
    label?: string;
    /** How often to ask whether the daemon is back. */
    pollMs?: number;
    /** How long to keep asking before giving up on an answer either way. */
    timeoutMs?: number;
  } = $props();

  type Phase = 'idle' | 'confirming' | 'restarting' | 'back' | 'never-left' | 'failed';

  let phase: Phase = $state('idle');
  /** The daemon's own words about what is about to happen. */
  let mechanismMessage = $state('');
  let failure = $state('');

  const STATUS: Record<Phase, string> = {
    idle: '',
    confirming: 'Every appliance reconnects.',
    restarting: '',
    back: 'Kessel is back.',
    'never-left': '',
    failed: '',
  };

  const status = $derived.by(() => {
    switch (phase) {
      case 'restarting':
        return mechanismMessage;
      case 'never-left':
        // The one message worth spelling out here rather than in the table: it is a diagnosis, and the
        // next thing the operator needs is where to look.
        return 'The restart did not happen. Check the add-on log.';
      case 'failed':
        return failure;
      default:
        return STATUS[phase];
    }
  });

  const sleep = (ms: number) =>
    new Promise<void>((resolve) => {
      setTimeout(resolve, ms);
    });

  /**
   * Polls until the daemon has gone and come back.
   *
   * "Gone" first, deliberately: a daemon that answered every poll never restarted, whatever the 202
   * said. A restart could in principle fit between two polls, but never seeing the daemon down is
   * reported as `never-left`, which is what a Supervisor refusing for want of `hassio_api: true` looks
   * like.
   *
   * Recursive rather than a `while` loop with an `await` in it: the polls are inherently sequential (each
   * one's result decides whether there is a next), which `no-await-in-loop` would flag, and `Promise.all`
   * does not apply.
   */
  function watchForRestart(deadline: number, sawItGo: boolean): Promise<void> {
    if (Date.now() >= deadline) {
      phase = sawItGo ? 'back' : 'never-left';
      return Promise.resolve();
    }
    return sleep(pollMs)
      .then(
        (): Promise<'back' | 'up' | 'gone'> =>
          health().then(
            () => (sawItGo ? 'back' : 'up'),
            () => 'gone',
          ),
      )
      .then((seen) => {
        if (seen === 'back') {
          phase = 'back';
          return;
        }
        return watchForRestart(deadline, sawItGo || seen === 'gone');
      });
  }

  async function press() {
    if (phase === 'idle' || phase === 'back' || phase === 'never-left' || phase === 'failed') {
      phase = 'confirming';
      return;
    }
    if (phase !== 'confirming') return;
    phase = 'restarting';
    failure = '';
    try {
      const outcome = await restart();
      mechanismMessage = outcome.message;
    } catch (error) {
      phase = 'failed';
      failure = error instanceof Error ? error.message : 'the restart could not be requested';
      return;
    }
    await watchForRestart(Date.now() + timeoutMs, false);
  }
</script>

<div class="flex flex-col gap-2">
  <button
    data-testid="restart-button"
    type="button"
    disabled={phase === 'restarting'}
    onclick={() => void press()}
    class="press inline-flex h-9 w-fit items-center gap-2 rounded-lg border px-3 text-sm font-medium disabled:opacity-50 {phase ===
    'confirming'
      ? 'border-destructive/40 text-destructive hover:bg-destructive/10'
      : 'hover:bg-muted'}"
  >
    {#if phase === 'restarting'}
      <LoaderCircle class="size-4 animate-spin" aria-hidden="true" />
    {:else}
      <Power class="size-4" aria-hidden="true" />
    {/if}
    {phase === 'confirming' ? 'Yes — restart now' : label}
  </button>

  {#if status}
    <p class="text-muted-foreground flex items-start gap-2 text-xs" data-testid="restart-status">
      {#if phase === 'back'}
        <CircleCheck class="mt-0.5 size-3.5 shrink-0 text-success" aria-hidden="true" />
      {:else if phase === 'failed' || phase === 'never-left'}
        <TriangleAlert class="text-destructive mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
      {/if}
      <span>{status}</span>
    </p>
  {/if}
</div>
