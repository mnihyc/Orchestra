/** Every mark the page makes has the one prefix, and a page that cannot make marks works as it did. */
import { afterEach, expect, it, vi } from 'vitest';

import { mark } from '@/lib/marks';

afterEach(() => { vi.unstubAllGlobals(); });

it('names every moment under the one prefix, with what it says of where it matters', () => {
  const made = vi.fn();
  vi.stubGlobal('performance', { now: () => 0, mark: made });
  mark('start/script');
  mark('switch/click', { tab: 'tab-1' });
  expect(made.mock.calls).toEqual([['orch/start/script', undefined], ['orch/switch/click', { detail: { tab: 'tab-1' } }]]);
});

it('makes nothing, and does not fail, where the browser has no marks', () => {
  vi.stubGlobal('performance', {});
  expect(() => { mark('start/script'); }).not.toThrow();
});
