/**
 * `FrameLog` is fed by a high-rate stream, so this suite pins both the accumulation contract
 * (dedupe, gaps, eviction) and its cost: an `append` that copies the whole ring per call makes the
 * inspector tab unresponsive at trace-level event rates.
 */
import { describe, expect, it } from 'vitest';
import type { TraceFrame } from '$lib/api-types';
import { FrameLog } from './frame-log';

/** A frame that differs from its neighbours only in `seq`, which is the only field this class reads. */
function frameAt(seq: number): TraceFrame {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000,
    kind: 'frame',
    level: 'trace',
    target: 'hc_proto::session',
    source: 'homeconnect',
    message: 'frame',
    fields: { direction: 'rx' },
    replayed: false,
  };
}

describe('frame_log_accumulates_a_firehose_without_lying_about_it', () => {
  it('drops a replayed seq rather than showing it twice', () => {
    const log = new FrameLog();

    log.append(frameAt(1));
    log.append(frameAt(2));
    log.append(frameAt(1));

    expect(log.frameCount).toBe(2);
    expect(log.entries.map((entry) => entry.row)).toEqual(['frame', 'frame']);
  });

  it('renders the hole where frames were lost, instead of a quietly shorter list', () => {
    const log = new FrameLog();

    log.append(frameAt(1));
    log.append(frameAt(5));

    const entries = log.entries;
    expect(entries.map((entry) => entry.row)).toEqual(['frame', 'gap', 'frame']);
    const gap = entries[1];
    expect(gap).toEqual({ row: 'gap', after: 1, before: 5, missing: 3 });
    // A gap is not a frame.
    expect(log.frameCount).toBe(2);
  });

  it('reports no gap on a filtered stream, because the holes are the operator’s own filter', () => {
    const log = new FrameLog({ detectGaps: false });

    log.append(frameAt(1));
    log.append(frameAt(90));

    expect(log.detectsGaps).toBe(false);
    expect(log.entries.map((entry) => entry.row)).toEqual(['frame', 'frame']);
  });

  it('evicts the oldest entry at capacity and does not leak the evicted seq', () => {
    const log = new FrameLog({ capacity: 3 });

    for (let seq = 1; seq <= 5; seq += 1) {
      log.append(frameAt(seq));
    }

    const entries = log.entries;
    expect(entries).toHaveLength(3);
    expect(entries.map((entry) => (entry.row === 'frame' ? entry.frame.seq : -1))).toEqual([
      3, 4, 5,
    ]);
    // `#seen` shrank with the ring, or a stream that runs for days would leak. The cost is that a
    // very late replay of an evicted frame reappears.
    expect(log.frameCount).toBe(3);
  });

  it('forgets everything on request', () => {
    const log = new FrameLog();
    log.append(frameAt(1));

    log.clear();

    expect(log.entries).toEqual([]);
    expect(log.frameCount).toBe(0);
  });

  /**
   * The cost assertion: an append's cost must not grow with the ring.
   *
   * A wall-clock budget would measure the machine, not the code, so this compares the same number of
   * appends at two capacities twentyfold apart. A constant-cost append gives a ratio near 1; one that
   * copies the ring gives a ratio near the capacity ratio. The 4x threshold sits well between them,
   * and both timings come from the same machine in the same second, so load cancels out.
   *
   * Each capacity is measured three times and the fastest run kept: a single garbage collection
   * inside one sample can blow the ratio, and a timing can be inflated by an unrelated pause but
   * never deflated.
   */
  it('appends at a cost that does not grow with the ring', () => {
    const appends = 20_000;
    const once = (capacity: number): number => {
      const log = new FrameLog({ capacity });
      const started = performance.now();
      for (let seq = 1; seq <= appends; seq += 1) {
        log.append(frameAt(seq));
      }
      const elapsed = performance.now() - started;
      // Read something off the log so no engine can decide the loop was dead code.
      expect(log.frameCount).toBe(Math.min(capacity, appends));
      return elapsed;
    };
    const cost = (capacity: number): number =>
      Math.min(once(capacity), once(capacity), once(capacity));

    // Warm the JIT on the same shape first, so the first measurement is not paying for compilation.
    once(10);
    const small = cost(100);
    const large = cost(2000);

    expect(large).toBeLessThan(Math.max(small, 1) * 4);
  });

  /**
   * `append` must not hand back a rendered array: a caller assigning it to `$state` on every event
   * would build a snapshot per event. The component takes snapshots once per redraw instead.
   */
  it('does not build a snapshot per append', () => {
    const log = new FrameLog();

    expect(log.append(frameAt(1))).toBeUndefined();
  });
});
