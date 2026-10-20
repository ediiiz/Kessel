<script lang="ts">
  /**
   * The feature browser as a sheet over the device page, opened from its "All features" row.
   * Sign-in sits at the top when the daemon asks for it, since that is what enables a change.
   */
  import type { EntityDescriptor, ProgramDescriptor } from '$lib/api-types';
  import type { ApiClient } from '$lib/api';
  import * as Sheet from '$lib/components/ui/sheet';
  import type { Writer } from '$lib/write.svelte';
  import FeatureBrowser from './FeatureBrowser.svelte';
  import SessionControl from './SessionControl.svelte';

  const {
    open,
    onopenchange,
    api,
    entities,
    programs,
    slug,
    role,
    write,
  }: {
    open: boolean;
    onopenchange: (open: boolean) => void;
    api: ApiClient;
    entities: EntityDescriptor[];
    programs: readonly ProgramDescriptor[];
    slug: string;
    role: string;
    write: Writer | null;
  } = $props();
</script>

<Sheet.Root {open} onOpenChange={onopenchange}>
  <Sheet.Content
    side="bottom"
    class="glass data-[side=bottom]:h-[92dvh] rounded-t-[1.75rem] border-x-0 border-b-0 px-4 pt-3 pb-0 sm:mx-auto sm:mb-6 sm:data-[side=bottom]:h-[85dvh] sm:max-w-2xl sm:rounded-[1.75rem] sm:border sm:px-6"
    data-testid="feature-sheet"
    onOpenAutoFocus={(event) => {
      // Focusing the search would raise a phone's keyboard over the list on every open.
      event.preventDefault();
    }}
  >
    <div class="bg-foreground/15 mx-auto h-1 w-10 shrink-0 rounded-full" aria-hidden="true"></div>
    <Sheet.Header class="p-0">
      <Sheet.Title class="text-xl font-semibold tracking-tight">All features</Sheet.Title>
      <Sheet.Description class="sr-only">Every feature this appliance reports.</Sheet.Description>
    </Sheet.Header>
    {#if write !== null}
      <SessionControl {api} />
    {/if}
    {#if open}
      <FeatureBrowser {entities} {programs} {slug} {role} {write} />
    {/if}
  </Sheet.Content>
</Sheet.Root>
