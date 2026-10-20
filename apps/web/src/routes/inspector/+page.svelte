<script lang="ts">
  /**
   * The live frame inspector route. `FrameInspector` owns the rendering and the dedupe; the stream
   * is a `GET` and carries no token.
   *
   * Direction, raw hex, decrypted JSON, `msgID` correlation, handshake step and crypto-chain state
   * can only arrive inside a frame's `fields`, and the daemon classifies an event as a frame by the
   * presence of a `direction` field.
   */
  import type { TraceFrame } from '$lib/api-types';
  import FrameInspector from '$lib/components/FrameInspector.svelte';
  import { subscribeFrames } from '$lib/live-shell';
  import type { FrameFilter } from '$lib/sse';
  import { enter } from '$lib/motion';

  /**
   * The inspector attaches to the app's one stream rather than opening a second, which would mean
   * two backlog replays and two parses of the same firehose. `$lib/live` owns the connection, opens
   * it at the broadest requirement of everyone attached and reopens it when that changes, so
   * narrowing the filter here still reaches the daemon, down to the floor the rest of the app needs
   * (see `subscribeFrames`).
   */
  function subscribe(onFrame: (frame: TraceFrame) => void, filter: FrameFilter): () => void {
    return subscribeFrames(onFrame, filter);
  }
</script>

<svelte:head>
  <title>Frame inspector — Kessel</title>
</svelte:head>

<div class="flex flex-col gap-5">
  <header class="pt-2" in:enter|global={{ index: 0 }}>
    <h1 class="text-[2.2rem] leading-tight font-semibold tracking-tight">Frames</h1>
  </header>

  <FrameInspector {subscribe} />
</div>
