<script lang="ts">
  /**
   * Where the operator pastes the API token.
   *
   * `type="password"` by default, plain text only behind an explicit reveal toggle, which is the
   * only place the token is ever visible.
   *
   * The value is an input value and nothing else: never text content, a `title`, a link attribute,
   * or a log line. `TokenField.test.ts` checks `textContent` and every `[title]`, so if you add a
   * tooltip here, do not interpolate the token into it.
   */
  import { Eye, EyeOff, KeyRound } from '$lib/icons';

  const {
    token,
    onchange,
  }: {
    token: string;
    onchange: (value: string) => void;
  } = $props();

  let revealed = $state(false);

  function edit(event: Event): void {
    const input = event.currentTarget;
    if (input instanceof HTMLInputElement) {
      onchange(input.value);
    }
  }

  function toggleReveal(): void {
    revealed = !revealed;
  }
</script>

<div class="flex flex-col gap-1.5">
  <label class="flex flex-col gap-1">
    <span class="text-muted-foreground flex items-center gap-1.5 text-xs font-medium">
      <KeyRound class="size-3.5" aria-hidden="true" />
      API token
    </span>
    <div class="flex gap-2">
      <input
        data-testid="token-input"
        type={revealed ? 'text' : 'password'}
        autocomplete="off"
        spellcheck="false"
        value={token}
        oninput={edit}
        class="border-input bg-background focus-visible:border-ring focus-visible:ring-ring/50 h-9 min-w-0 flex-1 rounded-lg border px-2.5 font-mono text-sm transition-colors focus-visible:ring-[3px] focus-visible:outline-none"
      />
      <button
        data-testid="token-reveal"
        type="button"
        onclick={toggleReveal}
        aria-label={revealed ? 'Hide the token' : 'Show the token'}
        class="press text-muted-foreground hover:bg-muted hover:text-foreground flex size-9 shrink-0 items-center justify-center rounded-lg border"
      >
        {#if revealed}
          <EyeOff class="size-4" aria-hidden="true" />
        {:else}
          <Eye class="size-4" aria-hidden="true" />
        {/if}
      </button>
    </div>
  </label>
</div>
