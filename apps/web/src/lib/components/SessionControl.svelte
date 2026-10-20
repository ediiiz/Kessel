<script lang="ts">
  /**
   * Sign in with the operator token, or sign out.
   *
   * The token is asked for once instead of once per tab: the exchange leaves an `HttpOnly` cookie
   * behind and the token is then deleted from `sessionStorage`, so the SPA stops holding an operator
   * credential.
   *
   * `TokenField` is still the input (it owns the password-plus-reveal behaviour), but it is only
   * rendered while there is no session.
   *
   * None of it is rendered unless the daemon authenticates, which it does not by default: the add-on
   * is reached through Home Assistant's ingress, which has already authenticated the browser. The
   * daemon reports `auth_required` on `GET /api/session`, and the operator turns it on in Settings.
   *
   * The signed-in state is a single quiet line, since it is the common case; the sign-in form is a
   * full card, because that is when the operator has something to do.
   */
  import type { ApiClient } from '$lib/api';
  import { CircleCheck, LogIn, LogOut, TriangleAlert } from '$lib/icons';
  import { clearToken, getToken, setToken } from '$lib/token';
  import { session, signIn, signOut } from '$lib/session.svelte';
  import TokenField from '$lib/components/TokenField.svelte';

  const { api }: { api: ApiClient } = $props();

  let token = $state(getToken() ?? '');
  let busy = $state(false);
  let failure = $state<string | null>(null);

  function edit(value: string): void {
    token = value;
    // Stored while it is being typed: an ingress navigation mid-sign-in is a full page load, which
    // would otherwise lose a half-pasted token. It is cleared the moment it is exchanged.
    setToken(value);
  }

  async function submit(): Promise<void> {
    busy = true;
    failure = null;
    try {
      await signIn(api, token);
      // The exchange succeeded, so this browser no longer needs, or keeps, the token.
      clearToken();
      token = '';
    } catch (error) {
      failure = error instanceof Error ? error.message : 'the token was not accepted';
    } finally {
      busy = false;
    }
  }

  async function leave(): Promise<void> {
    busy = true;
    failure = null;
    try {
      await signOut(api);
    } finally {
      busy = false;
    }
  }
</script>

{#if !session.authRequired}
  <!--
    Nothing at all. The daemon is not authenticating mutating requests — its default, because the add-on
    is reached through Home Assistant's ingress, which already authenticated this browser — so there is
    no session to hold and nothing to sign in to. An operator who wants the gate turns it on in Settings,
    and this control reappears.
  -->
{:else if session.authenticated}
  <p
    class="text-muted-foreground flex flex-wrap items-center gap-2 text-xs"
    data-testid="session-active"
  >
    <CircleCheck class="size-3.5 shrink-0 text-success" aria-hidden="true" />
    <span>Signed in</span>
    <button
      data-testid="session-sign-out"
      type="button"
      onclick={leave}
      disabled={busy}
      class="hover:text-foreground inline-flex items-center gap-1 font-medium underline underline-offset-4 transition-colors disabled:opacity-50"
    >
      <LogOut class="size-3" aria-hidden="true" />
      {busy ? 'Signing out…' : 'Sign out'}
    </button>
  </p>
{:else if session.checked}
  <div class="bg-card flex flex-col gap-3 rounded-xl border p-4" data-testid="session-sign-in">
    <div class="flex items-center gap-2">
      <LogIn class="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
      <h2 class="text-sm font-semibold tracking-tight">Sign in to enable writes</h2>
    </div>

    <TokenField {token} onchange={edit} />

    <div>
      <button
        data-testid="session-submit"
        type="button"
        onclick={submit}
        disabled={busy || token === ''}
        class="press bg-primary text-primary-foreground hover:bg-primary/90 inline-flex h-8 items-center rounded-lg px-3 text-sm font-medium disabled:opacity-50"
      >
        {busy ? 'Signing in…' : 'Sign in'}
      </button>
    </div>

    {#if failure !== null}
      <p
        class="border-destructive/40 bg-destructive/5 text-destructive flex items-start gap-2 rounded-lg border p-2.5 text-sm"
        data-testid="session-error"
      >
        <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <span>{failure}</span>
      </p>
    {/if}
  </div>
{/if}
