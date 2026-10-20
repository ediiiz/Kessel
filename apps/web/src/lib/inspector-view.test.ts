import { describe, expect, it } from 'vitest';
import { frameTime } from './inspector-view';

describe('frameTime', () => {
  it('shows local wall-clock time with milliseconds, zero-padded', () => {
    const at = new Date(2026, 9, 20, 7, 3, 9, 42).getTime();
    expect(frameTime(at)).toBe('07:03:09.042');
  });
});
