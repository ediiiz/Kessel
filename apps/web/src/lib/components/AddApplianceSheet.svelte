<script lang="ts">
  /**
   * "Add appliance" as a sheet over the current screen: upload the profile ZIP, confirm, done.
   * The wizard and its confirmation are unchanged; `onadded` lets the page refresh its list.
   */
  import type { CommitRequest, OnboardingResult, ProfilePreview } from '$lib/api-types';
  import type { ApiClient } from '$lib/api';
  import * as Sheet from '$lib/components/ui/sheet';
  import OnboardingWizard from './OnboardingWizard.svelte';
  import SessionControl from './SessionControl.svelte';

  const {
    open,
    onopenchange,
    api,
    onadded,
  }: {
    open: boolean;
    onopenchange: (open: boolean) => void;
    api: ApiClient;
    onadded?: () => void;
  } = $props();

  const preview = (zip: File): Promise<ProfilePreview> => api.previewProfile(zip);
  async function commit(zip: File, request: CommitRequest): Promise<OnboardingResult> {
    const result = await api.commitProfile(zip, request);
    onadded?.();
    return result;
  }
  const setManualHost = (slug: string, host: string): Promise<OnboardingResult> =>
    api.setManualHost(slug, host);
</script>

<Sheet.Root {open} onOpenChange={onopenchange}>
  <Sheet.Content
    side="bottom"
    class="glass max-h-[92dvh] rounded-t-[1.75rem] border-x-0 border-b-0 px-5 pt-3 pb-0 sm:mx-auto sm:mb-6 sm:max-w-xl sm:rounded-[1.75rem] sm:border"
    data-testid="add-appliance-sheet"
  >
    <div class="bg-foreground/15 mx-auto h-1 w-10 rounded-full" aria-hidden="true"></div>
    <Sheet.Header class="p-0">
      <Sheet.Title class="text-xl font-semibold tracking-tight">Add appliance</Sheet.Title>
      <Sheet.Description class="sr-only">Upload the appliance's profile ZIP.</Sheet.Description>
    </Sheet.Header>
    <div
      class="no-scrollbar scroll-fade -mx-5 flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 pb-[calc(1.5rem+env(safe-area-inset-bottom))]"
    >
      <SessionControl {api} />
      {#if open}
        <OnboardingWizard {preview} {commit} {setManualHost} />
      {/if}
    </div>
  </Sheet.Content>
</Sheet.Root>
