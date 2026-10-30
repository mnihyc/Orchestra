/** The VNC view's rules: why a display's socket ended, read from its close frame; quality lowered when updates fall behind and raised slowly when they keep up; the line a program starts with; a size as a display takes it. */
import { describe, expect, it } from 'vitest';

import { adapt, displayLine, endOf, FIRST_QUALITY, GEOMETRY, QUALITY_BEST, QUALITY_LEAST, type QualityState } from '@/features/vnc/viewer';

describe('why a display socket ended is in its close frame', () => {
  it('reads the reasons the server names, any other as unknown, and another code as a lost line', () => {
    expect(endOf(4000, 'closed_by_agent')).toBe('closed_by_agent');
    expect(endOf(4000, 'protocol')).toBe('protocol');
    expect(endOf(4000, 'something new')).toBe('unknown');
    expect(endOf(1006, '')).toBeNull();
  });
});

describe('the picture asks for less when updates fall behind', () => {
  const run = (seconds: boolean[], from: QualityState = FIRST_QUALITY) => seconds.reduce(adapt, from);
  it('passes over one slow second and steps down after two', () => {
    expect(run([true]).level).toBe(QUALITY_BEST);
    expect(run([true, true]).level).toBe(QUALITY_BEST - 2);
    expect(run([true, false, true]).level).toBe(QUALITY_BEST);
  });
  it('never asks for less than the least and climbs back one level after ten seconds that keep up', () => {
    const low = run(Array.from({ length: 20 }, () => true));
    expect(low.level).toBe(QUALITY_LEAST);
    expect(run(Array.from({ length: 9 }, () => false), low).level).toBe(QUALITY_LEAST);
    expect(run(Array.from({ length: 10 }, () => false), low).level).toBe(QUALITY_LEAST + 1);
    expect(run(Array.from({ length: 200 }, () => false), low).level).toBe(QUALITY_BEST);
  });
});

describe('what a display shows people', () => {
  it('gives the line a program is started with', () => {
    expect(displayLine(':1')).toBe('DISPLAY=:1');
  });
  it('takes a size as width by height in pixels', () => {
    expect(GEOMETRY.test('1280x800')).toBe(true);
    expect(GEOMETRY.test('1280 x 800')).toBe(false);
    expect(GEOMETRY.test('0x800')).toBe(false);
  });
});
