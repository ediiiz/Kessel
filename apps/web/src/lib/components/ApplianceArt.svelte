<script lang="ts">
  /**
   * A role's artwork: a tinted scene with stacked translucent panels and the role's glyph on a
   * frosted disc. Generated, so it implies nothing about which physical appliance this is.
   */
  import { roleHue } from '$lib/appliance';
  import { cn } from '$lib/utils.js';
  import ApplianceIcon from './ApplianceIcon.svelte';

  const {
    role,
    size = 'card',
    class: className = '',
  }: { role: string; size?: 'card' | 'hero'; class?: string } = $props();
</script>

<div class={cn('scene', className)} style:--hue={roleHue(role)} aria-hidden="true">
  <div
    class={cn(
      'absolute z-10 rounded-[1.75rem] border border-white/15 bg-white/[0.06] backdrop-blur-[2px]',
      size === 'hero' ? 'top-6 -right-4 h-48 w-36 rotate-6' : 'top-4 right-4 h-28 w-20 rotate-6',
    )}
  ></div>
  <div
    class={cn(
      'absolute z-10 rounded-[1.75rem] border border-white/20 bg-white/[0.09] backdrop-blur-sm',
      size === 'hero'
        ? 'top-16 right-20 h-48 w-36 -rotate-3'
        : 'top-7 right-11 h-24 w-[4.5rem] -rotate-3',
    )}
  ></div>
  <div
    class={cn(
      'absolute z-20 flex items-center justify-center rounded-full border border-white/30 bg-white/15 text-white shadow-[0_8px_30px_-6px_rgb(0_0_0/0.35)] backdrop-blur-md',
      size === 'hero' ? 'top-20 right-16 size-24' : 'top-10 right-12 size-14',
    )}
  >
    <ApplianceIcon {role} class={size === 'hero' ? 'size-11' : 'size-7'} />
  </div>
</div>
