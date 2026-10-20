/**
 * The one way the SPA sends a write: proposed and confirmed ({@link WriteGate.propose} then
 * {@link WriteGate.confirm}), or sent straight away ({@link WriteGate.send}) for a quick setting on
 * an appliance that does not produce heat.
 *
 * This is UI sequencing, not a security boundary: the daemon carries a write to any appliance.
 *
 * {@link WriteGate.confirm} sends once and reports what came back, including a refusal, verbatim.
 * There is no retry here or anywhere above it: a failed write against a cooktop that may have
 * partially applied is not a thing to quietly try again.
 *
 * The outcome is reported here, as a toast, rather than by the component that proposed the write,
 * so no entry point can forget to report it. A toast because the confirmation modal closes on
 * commit and on a phone the proposing control is often scrolled away by the time the daemon
 * answers; both toasts carry the feature key and value so they stand alone.
 *
 * Sending and being accepted are announced separately. The round trip to the appliance has no upper
 * bound, and silence over it reads as a dropped write, which invites a second send. The in-flight
 * toast is replaced by the verdict rather than stacked, since two toasts read as two writes.
 */
import { toast } from 'svelte-sonner';
import type { WriteOutcome } from '$lib/api-types';

/** A write the operator has been shown but has not yet committed. */
export interface ProposedWrite {
  /** The full dotted feature key, exactly as it will be sent. */
  readonly featureKey: string;
  /** The value, exactly as it will be sent. */
  readonly value: string;
  /**
   * Optional operator-facing name for what this does ("Power → On"), shown in place of the raw key
   * and value. Cosmetic: the key and value above are what is sent.
   */
  readonly label?: string;
}

/** How a write is described to the operator: its label, or the raw key and value. */
export function describeWrite(write: ProposedWrite): string {
  return write.label ?? `${write.featureKey} → ${write.value}`;
}

/** The device-page callback that actually reaches `POST /api/devices/{slug}/write`. */
export type Writer = (
  slug: string,
  request: { feature_key: string; value: string },
) => Promise<WriteOutcome>;

export class WriteGate {
  /** The write awaiting confirmation, or `null`. Non-null is what puts the confirmation on screen. */
  pending = $state<ProposedWrite | null>(null);
  /** In flight. Disables both buttons so a double-tap cannot send twice. */
  busy = $state(false);
  /** The write in flight, so the control that started it can show it as pending. */
  sending = $state<ProposedWrite | null>(null);

  /** Offers a write for confirmation. Sends nothing. */
  propose(write: ProposedWrite): void {
    if (this.busy) {
      return;
    }
    this.pending = write;
  }

  /** Drops the proposal. Sends nothing. */
  cancel(): void {
    if (this.busy) {
      return;
    }
    this.pending = null;
  }

  /**
   * Commits the pending write: one attempt, then the outcome.
   *
   * The proposal is cleared before awaiting, so the confirmation cannot be clicked a second time
   * while the first request is in flight; `busy` guards the same thing.
   */
  async confirm(slug: string, write: Writer): Promise<void> {
    const proposed = this.pending;
    if (proposed === null) {
      return;
    }
    await this.send(slug, write, proposed);
  }

  /** Sends `proposed` now, without a confirmation: one attempt, then the outcome. */
  async send(slug: string, write: Writer, proposed: ProposedWrite): Promise<void> {
    if (this.busy) {
      return;
    }
    this.busy = true;
    this.pending = null;
    this.sending = proposed;
    // The proposal is gone by the time the daemon answers, and a toast that says only "refused"
    // is not a report.
    const description = describeWrite(proposed);
    // `toast.loading` does not expire on its own, so it can stand for an unbounded wait; it is
    // dismissed below on every path out.
    const sending = toast.loading(description, { description: 'Sending to the appliance…' });
    try {
      const outcome = await write(slug, {
        feature_key: proposed.featureKey,
        value: proposed.value,
      });
      if (outcome.outcome === 'accepted') {
        toast.success(description, { description: 'Applied.' });
      } else {
        // The daemon's prose, verbatim. Not summarised, not softened.
        toast.error(`Not applied: ${description}`, { description: outcome.reason });
      }
    } catch (error) {
      // No retry, here or anywhere: a failed write is reported and left alone.
      toast.error(`Not sent: ${description}`, {
        description: error instanceof Error ? error.message : 'The write could not be sent.',
      });
    } finally {
      // Replaced, not stacked, so it does not read as two writes. In `finally` so no path leaves it
      // spinning.
      toast.dismiss(sending);
      this.busy = false;
      this.sending = null;
    }
  }
}
