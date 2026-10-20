<script lang="ts">
  /**
   * The confirmation step before a write leaves the browser, shared by `QuickActions` (on a heating
   * appliance) and `FeatureBrowser`. The sequencing lives in `$lib/write.svelte.ts`; the outcome is a
   * toast raised there. The dialog is controlled by {@link WriteGate.pending}, so the gate is the one
   * answer to "is a write awaiting confirmation".
   */
  import { Flame, Send } from '$lib/icons';
  import { isHeating } from '$lib/appliance';
  import * as Dialog from '$lib/components/ui/dialog';
  import { describeWrite, type WriteGate, type Writer } from '$lib/write.svelte';

  const {
    gate,
    slug,
    role,
    write,
  }: {
    gate: WriteGate;
    slug: string;
    role: string;
    write: Writer;
  } = $props();
</script>

<!-- Every way out that is not "Send" calls `cancel()`. `confirm()` sets `busy` before it clears
     `pending`, so the close it causes cannot cancel the write it started. -->
<Dialog.Root
  open={gate.pending !== null}
  onOpenChange={(open) => {
    if (!open) {
      gate.cancel();
    }
  }}
>
  <Dialog.Content class="sm:max-w-md" data-testid="write-confirm-panel" showCloseButton={false}>
    {#if gate.pending !== null}
      {@const pending = gate.pending}
      <Dialog.Header>
        <Dialog.Title>Send to the appliance?</Dialog.Title>
        <Dialog.Description class="sr-only">Confirm the change.</Dialog.Description>
      </Dialog.Header>

      <div class="bg-muted/40 flex flex-col gap-1 rounded-lg p-3" data-testid="write-pending">
        <p class="text-sm font-semibold break-words" data-testid="write-pending-label">
          {describeWrite(pending)}
        </p>
        <p class="text-muted-foreground font-mono text-[0.7rem] break-all">
          {slug} · {pending.featureKey} = {pending.value}
        </p>
      </div>

      {#if isHeating(role)}
        <p
          class="text-destructive flex items-start gap-2 text-xs font-medium"
          data-testid="write-heat-warning"
        >
          <Flame class="mt-px size-3.5 shrink-0" aria-hidden="true" />
          <span> This appliance produces heat. </span>
        </p>
      {/if}

      <Dialog.Footer>
        <button
          type="button"
          data-testid="write-cancel"
          disabled={gate.busy}
          onclick={() => gate.cancel()}
          class="press hover:bg-muted inline-flex h-8 items-center rounded-lg border px-3 text-sm font-medium disabled:opacity-50"
        >
          Cancel
        </button>
        <button
          type="button"
          data-testid="write-confirm"
          disabled={gate.busy}
          onclick={() => void gate.confirm(slug, write)}
          class="press bg-primary text-primary-foreground hover:bg-primary/90 inline-flex h-8 items-center gap-1.5 rounded-lg px-3 text-sm font-medium disabled:opacity-50"
        >
          <Send class="size-3.5" aria-hidden="true" />
          {gate.busy ? 'Sending…' : 'Send'}
        </button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
