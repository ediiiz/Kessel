<script lang="ts">
  /**
   * The settings page: loads the stored configuration and renders {@link ConfigForm}. The daemon
   * never returns the broker password; see the form for how a save avoids wiping it.
   *
   * `{#await}` keeps the failure path in the markup beside the success one and avoids a floating
   * promise.
   *
   * It also shows the daemon's name and version, which describe the add-on rather than any
   * appliance. That read is not blocking: a daemon that cannot answer `GET /api/health` costs this
   * page a line of text, not its form.
   */
  import { TriangleAlert } from '$lib/icons';
  import { Skeleton } from '$lib/components/ui/skeleton';
  import { enter, leave } from '$lib/motion';
  import ConfigForm from '$lib/components/ConfigForm.svelte';
  import { createApiClient } from '$lib/api';
  import { session, refresh } from '$lib/session.svelte';
  import { getToken } from '$lib/token';

  // Both credentials are offered, as on every page that writes: a token still in `sessionStorage` and
  // the session cookie. Without `hasSession` the client would refuse every save locally, even when
  // `require_auth` is off and the daemon would accept it.
  const client = createApiClient({
    token: () => getToken(),
    hasSession: () => session.authenticated,
  });
  const loading = client.config();

  $effect(() => {
    // A read: `GET /api/session` needs no credential. It answers `authenticated: true` whenever the
    // daemon does not authenticate at all, which is what lets a save through on the default settings.
    void refresh(client);
  });

  /**
   * The daemon's identity, for the version line under the heading.
   *
   * Handled in the markup like {@link loading}: the failure branch is an `{:catch}` that renders
   * nothing, which neither chains (`unicorn(prefer-top-level-await)`) nor leaves the rejection
   * floating.
   */
  const build = client.health();
</script>

<svelte:head>
  <title>Settings — Kessel</title>
</svelte:head>

<div class="flex flex-col gap-5">
  <header in:enter|global={{ index: 0 }}>
    <h1 class="text-2xl font-semibold tracking-tight">Settings</h1>
    {#await build then health}
      <p class="text-muted-foreground mt-1 font-mono text-xs" data-testid="build-version">
        {health.name}
        {health.version}
      </p>
    {:catch}
      <!-- A daemon that cannot answer `GET /api/health` costs this page a line of text, not its form.
           The settings request has its own error branch below, which is the one worth showing. -->
    {/await}
  </header>

  <div class="stack">
    {#await loading}
      <div
        class="flex max-w-2xl flex-col gap-3"
        aria-busy="true"
        in:enter|global={{ index: 1 }}
        out:leave
      >
        <Skeleton class="h-48 rounded-xl" />
        <Skeleton class="h-24 rounded-xl" />
      </div>
    {:then view}
      <div in:enter|global={{ index: 1 }}>
        <ConfigForm
          {view}
          save={(update) => client.saveConfig(update)}
          restart={() => client.restart()}
          health={() => client.health()}
        />
      </div>
    {:catch error}
      <p
        class="border-destructive/40 bg-destructive/5 text-destructive flex max-w-2xl items-start gap-2 rounded-xl border p-3 text-sm"
        data-testid="config-load-error"
        in:enter|global={{ index: 1 }}
      >
        <TriangleAlert class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <span>{error instanceof Error ? error.message : 'the settings could not be loaded'}</span>
      </p>
    {/await}
  </div>
</div>
