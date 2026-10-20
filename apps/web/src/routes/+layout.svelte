<script lang="ts">
  import type { Snippet } from 'svelte';
  import { page } from '$app/state';
  import { link } from '$lib/links';
  import Logo from '$lib/components/Logo.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import * as Sidebar from '$lib/components/ui/sidebar';
  import { Toaster } from '$lib/components/ui/sonner';
  import { Activity, Check, House, Monitor, Moon, Settings, Sun } from '$lib/icons';
  import { startLive } from '$lib/live-shell';
  import { APP_NAME, ROUTES } from '$lib/index.js';
  import { enter } from '$lib/motion';
  import { applyTheme, setTheme, startTheme, theme, type ThemeChoice } from '$lib/theme.svelte';
  import { cn } from '$lib/utils.js';
  import '../app.css';

  const { children }: { children: Snippet } = $props();

  const pane = $state<{ element: HTMLElement | null }>({ element: null });

  const DESTINATIONS = [
    { id: ROUTES.home, label: 'Home', icon: House },
    { id: ROUTES.inspector, label: 'Frames', icon: Activity },
    { id: ROUTES.config, label: 'Settings', icon: Settings },
  ] as const;

  function isActive(id: string): boolean {
    const current = page.route.id ?? ROUTES.home;
    return id === ROUTES.home
      ? current === ROUTES.home || current === ROUTES.device
      : current === id;
  }

  const WIDE_ROUTES = new Set<string>([ROUTES.device, ROUTES.inspector]);
  const wide = $derived(WIDE_ROUTES.has(page.route.id ?? ''));
  /** The device page draws its own full-bleed hero, so the phone top bar steps aside. */
  const bleed = $derived(page.route.id === ROUTES.device);
  /** Home and device pages carry their own headers on a phone. */
  const bare = $derived(bleed || page.route.id === ROUTES.home);

  const THEME_ICONS = { light: Sun, dark: Moon, system: Monitor };
  const THEME_CHOICES: readonly { readonly id: ThemeChoice; readonly label: string }[] = [
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
    { id: 'system', label: 'System' },
  ];
  const ThemeIcon = $derived(THEME_ICONS[theme.choice]);
  const themeTitle = $derived(
    theme.choice === 'system'
      ? `Theme: following the system (${theme.systemDark ? 'dark' : 'light'})`
      : `Theme: ${theme.choice}`,
  );

  $effect(() => {
    // Re-runs on a route change only: navigating between two appliances keeps the scroll position,
    // since the operator is comparing them.
    void page.route.id;
    pane.element?.scrollTo({ top: 0 });
  });

  $effect(() => {
    // The media-query listener that keeps `'system'` live, detached on teardown.
    return startTheme();
  });

  $effect(() => {
    // The app's one live stream, closed when the shell goes away. Reads only: SSE is a `GET`, and
    // nothing on this stream can reach an appliance.
    return startLive();
  });

  $effect(() => {
    // Re-runs whenever the choice or the system preference changes, because `theme.dark` reads both.
    applyTheme();
  });
</script>

<Toaster position="top-center" richColors closeButton />

<div class="grain" aria-hidden="true"></div>

<!-- Fixed height: the pane inside scrolls, not the document. -->
<Sidebar.Provider class="relative z-10 h-[100dvh] min-h-0 overflow-hidden">
  <Sidebar.Root collapsible="icon" class="border-none">
    <Sidebar.Header class="glass border-b">
      <Sidebar.Menu>
        <Sidebar.MenuItem>
          <Sidebar.MenuButton size="lg" class="gap-3">
            {#snippet child({ props })}
              <a href={link(ROUTES.home)} {...props}>
                <Logo class="size-7" />
                <span class="text-base font-semibold tracking-tight">{APP_NAME}</span>
              </a>
            {/snippet}
          </Sidebar.MenuButton>
        </Sidebar.MenuItem>
      </Sidebar.Menu>
    </Sidebar.Header>

    <Sidebar.Content class="glass">
      <Sidebar.Group>
        <Sidebar.GroupContent>
          <Sidebar.Menu>
            {#each DESTINATIONS as destination (destination.id)}
              {@const active = isActive(destination.id)}
              <Sidebar.MenuItem>
                <!-- `tooltipContent` is what makes the icon-collapsed rail usable: collapsed to
                     3rem there is no label, so the name has to arrive on hover. -->
                <Sidebar.MenuButton isActive={active} tooltipContent={destination.label}>
                  {#snippet child({ props })}
                    <a
                      href={link(destination.id)}
                      aria-current={active ? 'page' : undefined}
                      {...props}
                    >
                      <destination.icon aria-hidden="true" />
                      <span>{destination.label}</span>
                    </a>
                  {/snippet}
                </Sidebar.MenuButton>
              </Sidebar.MenuItem>
            {/each}
          </Sidebar.Menu>
        </Sidebar.GroupContent>
      </Sidebar.Group>
    </Sidebar.Content>

    <Sidebar.Footer class="glass border-t">
      <Sidebar.Menu>
        <Sidebar.MenuItem>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger>
              {#snippet child({ props })}
                <Sidebar.MenuButton {...props} tooltipContent={themeTitle}>
                  <ThemeIcon aria-hidden="true" />
                  <span class="capitalize">{theme.choice}</span>
                </Sidebar.MenuButton>
              {/snippet}
            </DropdownMenu.Trigger>
            <DropdownMenu.Content side="top" align="start" class="w-40">
              <DropdownMenu.Group>
                {#each THEME_CHOICES as choice (choice.id)}
                  <DropdownMenu.Item
                    onSelect={() => {
                      setTheme(choice.id);
                    }}
                  >
                    {@const Icon = THEME_ICONS[choice.id]}
                    <Icon aria-hidden="true" />
                    <span>{choice.label}</span>
                    {#if theme.choice === choice.id}
                      <Check class="ml-auto" aria-hidden="true" />
                    {/if}
                  </DropdownMenu.Item>
                {/each}
              </DropdownMenu.Group>
            </DropdownMenu.Content>
          </DropdownMenu.Root>
        </Sidebar.MenuItem>
      </Sidebar.Menu>
    </Sidebar.Footer>

    <!-- The collapse affordance: a thin clickable strip on the sidebar's edge, so the toggle does not
         need a button competing with the brand. ⌘/Ctrl-B also works, from the provider. -->
    <Sidebar.Rail />
  </Sidebar.Root>

  <!-- The `<main>`, and a flex *column* rather than the scroller — the scroller is the div inside. -->
  <Sidebar.Inset class="min-h-0 min-w-0 overflow-hidden">
    <!-- Phone top bar: the brand and the theme control, which the desktop sidebar carries. -->
    <header
      class={cn(
        'glass z-20 flex h-14 shrink-0 items-center justify-between rounded-none border-0 border-b px-4 shadow-none md:hidden',
        bare && 'hidden',
      )}
    >
      <a href={link(ROUTES.home)} class="flex items-center gap-2 font-semibold tracking-tight">
        <Logo class="size-6" />
        {APP_NAME}
      </a>

      <DropdownMenu.Root>
        <DropdownMenu.Trigger
          title={themeTitle}
          aria-label={themeTitle}
          class="press text-muted-foreground hover:bg-muted hover:text-foreground flex size-9 items-center justify-center rounded-lg"
        >
          <ThemeIcon class="size-4" aria-hidden="true" />
        </DropdownMenu.Trigger>
        <DropdownMenu.Content align="end" class="w-40">
          <DropdownMenu.Group>
            {#each THEME_CHOICES as choice (choice.id)}
              <DropdownMenu.Item
                onSelect={() => {
                  setTheme(choice.id);
                }}
              >
                {@const Icon = THEME_ICONS[choice.id]}
                <Icon aria-hidden="true" />
                <span>{choice.label}</span>
                {#if theme.choice === choice.id}
                  <Check class="ml-auto" aria-hidden="true" />
                {/if}
              </DropdownMenu.Item>
            {/each}
          </DropdownMenu.Group>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    </header>

    <!-- The only scrolling pane. `min-h-0` is what allows it to scroll inside a flex column. -->
    <div
      class="no-scrollbar scroll-fade relative min-h-0 flex-1 overflow-y-auto"
      bind:this={pane.element}
    >
      <!--
        The route transition.

        Keyed on `page.route.id`, not the URL: navigating between two appliances is a parameter
        change on one route, and the device page keeps the previous appliance on screen until the new
        data lands (see `blank` there). Keying on the URL would remount the page and undo that.
      -->
      {#key page.route.id}
        <div
          class={cn(
            'mx-auto w-full px-4 pt-5 pb-28 md:px-8 md:py-8',
            wide ? 'max-w-[100rem]' : 'max-w-5xl',
            bleed && 'pt-0',
          )}
          in:enter|global={{ y: 12 }}
        >
          {@render children()}
        </div>
      {/key}
    </div>

    <nav
      class="glass fixed inset-x-4 bottom-[calc(0.75rem+env(safe-area-inset-bottom))] z-40 grid grid-cols-3 rounded-full border p-1.5 md:hidden"
      aria-label="Primary"
    >
      {#each DESTINATIONS as destination (destination.id)}
        {@const active = isActive(destination.id)}
        <a
          href={link(destination.id)}
          aria-current={active ? 'page' : undefined}
          class={cn(
            'press flex flex-col items-center gap-0.5 rounded-full py-2 text-[0.7rem] font-medium',
            active ? 'bg-primary/12 text-primary' : 'text-muted-foreground',
          )}
        >
          <destination.icon class="size-5" aria-hidden="true" />
          {destination.label}
        </a>
      {/each}
    </nav>
  </Sidebar.Inset>
</Sidebar.Provider>
