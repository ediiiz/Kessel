/**
 * What the inspector accumulates from the SSE stream.
 *
 * The stream is not a clean append-only feed: `EventSource` reconnects on its own, and
 * `TraceFrame.seq` is monotonic and gap-free, so a gap means frames were dropped. This module is
 * the only place that handles either case:
 *
 *   1. A replayed frame must not appear twice; `seq` is the identity.
 *   2. A missing frame must not vanish quietly. A gap becomes a rendered entry, so the operator
 *      sees "4 events were dropped here" rather than a silently shorter list. When diagnosing a
 *      handshake, a hole in the sequence can itself be the finding.
 *
 * Both a reconnect that replays and one that resumes from the live tail are handled, so this code
 * does not depend on whether the daemon honours `Last-Event-ID`.
 *
 * Appending must be constant-cost: at trace level the stream runs at a couple of hundred events
 * per second, and this class is on that path once per event. So the storage is a fixed-length ring
 * with a write cursor, and [`append`] touches exactly one slot. It returns nothing; materialising
 * the rows is [`entries`], which is O(capacity) and meant to be read once per rendered frame.
 */
import type { TraceFrame } from '$lib/api-types';

/** One rendered row: either a frame, or the hole where frames were lost. */
export type LogEntry =
  | { readonly row: 'frame'; readonly frame: TraceFrame }
  | {
      readonly row: 'gap';
      readonly after: number;
      readonly before: number;
      readonly missing: number;
    };

/** How many entries the inspector keeps before evicting the oldest. */
export const DEFAULT_CAPACITY = 1000;

/** How a log is built. */
export interface LogOptions {
  /** How many entries to keep. */
  readonly capacity?: number;
  /**
   * Whether a hole in `seq` means "dropped" — **false whenever the stream is filtered**.
   *
   * `seq` is assigned before the daemon applies the `?level=`/`?source=` filter, so on a filtered
   * stream the holes are the events the operator excluded, not data loss. The UI says so: with a
   * filter on, a genuine drop is not detectable here.
   */
  readonly detectGaps?: boolean;
}

export class FrameLog {
  readonly #capacity: number;
  readonly #detectGaps: boolean;
  /**
   * The ring. A fixed-length array of slots; `undefined` marks one never yet written.
   *
   * Allocated at full capacity up front so the array's length never changes and no slot is ever
   * moved. `#write` is the next slot to fill and `#size` how many are live, which together give the
   * read order without any shifting.
   */
  #slots: (LogEntry | undefined)[];
  #write = 0;
  #size = 0;
  #seen = new Set<number>();
  #highest: number | null = null;

  constructor(options: LogOptions = {}) {
    this.#capacity = Math.max(1, options.capacity ?? DEFAULT_CAPACITY);
    this.#detectGaps = options.detectGaps ?? true;
    this.#slots = Array.from({ length: this.#capacity });
  }

  /** Whether this log reports holes in `seq` as dropped events. */
  get detectsGaps(): boolean {
    return this.#detectGaps;
  }

  /**
   * The rows to render, oldest first.
   *
   * O(capacity): read this once per rendered frame, never once per event. It returns a fresh array,
   * both because the ring's slot order is not the read order and because a component assigning it to
   * `$state` needs a new identity to re-render.
   */
  get entries(): readonly LogEntry[] {
    const ordered: LogEntry[] = [];
    // Where the oldest live entry sits: `#write` once the ring has wrapped, slot 0 before that.
    const oldest = this.#size === this.#capacity ? this.#write : 0;
    for (let step = 0; step < this.#size; step += 1) {
      const entry = this.#slots[(oldest + step) % this.#capacity];
      if (entry !== undefined) {
        ordered.push(entry);
      }
    }
    return ordered;
  }

  /** How many distinct frames are held (gaps are not frames, and an evicted frame is not held). */
  get frameCount(): number {
    return this.#seen.size;
  }

  /**
   * Accepts one frame. A `seq` already seen is dropped; a `seq` beyond the next expected one adds a
   * gap entry first.
   *
   * Returns nothing; render from [`entries`], once per frame.
   */
  append(frame: TraceFrame): void {
    if (this.#seen.has(frame.seq)) {
      return;
    }
    if (this.#detectGaps && this.#highest !== null && frame.seq > this.#highest + 1) {
      this.#push({
        row: 'gap',
        after: this.#highest,
        before: frame.seq,
        missing: frame.seq - this.#highest - 1,
      });
    }
    this.#seen.add(frame.seq);
    this.#highest = this.#highest === null ? frame.seq : Math.max(this.#highest, frame.seq);
    this.#push({ row: 'frame', frame });
  }

  /**
   * Forgets everything. Used when the operator asks for a clean slate.
   *
   * A fresh slot array rather than an in-place wipe drops the references to every retained frame at
   * once instead of holding them until they are overwritten.
   */
  clear(): void {
    this.#slots = Array.from({ length: this.#capacity });
    this.#write = 0;
    this.#size = 0;
    this.#seen = new Set<number>();
    this.#highest = null;
  }

  /** One slot written, one possibly evicted. No allocation, no copying, no growth. */
  #push(entry: LogEntry): void {
    const evicted = this.#slots[this.#write];
    if (evicted?.row === 'frame') {
      // Keeping the seq in `#seen` would leak memory. Forgetting it means a very late replay of an
      // evicted frame could reappear, which is the better failure.
      this.#seen.delete(evicted.frame.seq);
    }
    this.#slots[this.#write] = entry;
    this.#write = (this.#write + 1) % this.#capacity;
    this.#size = Math.min(this.#size + 1, this.#capacity);
  }
}
