/** What a thread's frame changed of how it runs: the composer's facts (its tool access line among them) are read again when
 * the thread's own choices change, and when a block clears, which a change elsewhere (a place's setting) mended. */
import { expect, it } from 'vitest';

import type { Thread } from '@/generated/api-types';
import { choicesChanged } from '@/realtime/live';

const thread = (more: Partial<Thread> = {}): Thread =>
  ({ id: 'thread-1', model: 'gpt-6-luna', effort: null, execution_revision: 4, state: 'idle', ...more }) as unknown as Thread;

it('reads the choices again when a block clears, whatever the thread goes on to', () => {
  const blocked = thread({ state: 'blocked' });
  for (const state of ['idle', 'starting', 'running'] as const) expect(choicesChanged(blocked, thread({ state }), false)).toBe(true);
});

it('reads them again when the thread itself changed how it runs, and not for a frame that changed none of it', () => {
  expect(choicesChanged(thread(), thread({ model: 'gpt-5-mini' }), false)).toBe(true);
  expect(choicesChanged(thread(), thread({ effort: 'low' }), false)).toBe(true);
  expect(choicesChanged(thread(), thread({ execution_revision: 5 }), false)).toBe(true);
  expect(choicesChanged(thread(), thread(), true)).toBe(true);
  expect(choicesChanged(thread(), thread({ state: 'running' }), false)).toBe(false);
  expect(choicesChanged(thread({ state: 'running' }), thread({ state: 'idle' }), false)).toBe(false);
  // A block that stays, or one that begins, says its own words in the thread's line: nothing more is read.
  expect(choicesChanged(thread({ state: 'blocked' }), thread({ state: 'blocked' }), false)).toBe(false);
  expect(choicesChanged(thread(), thread({ state: 'blocked' }), false)).toBe(false);
});
