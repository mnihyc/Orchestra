/** Loaded metadata stays cheap across a thousand active threads and deeply nested places. */
import { expect, it } from 'vitest';
import { containedThreadStates, THREAD_URGENCY } from '@/lib/threadDisplay';
import type { TreeNode } from '@/generated/api-types';
import { acmeTree, ACME } from '../doubles/fixtures';
import { display } from '../doubles/thread-display';

// The scale bar includes a thousand active threads; a deep parent chain exposes repeated ancestor work.
const THREADS = 1_000;
const PLACES = 1_000;

class MeasuredTree extends Map<string, TreeNode> {
  reads = 0;
  override get(id: string): TreeNode | undefined {
    this.reads++;
    return super.get(id);
  }
}

it('folds a thousand threads through deep places with work bounded by nodes and state priorities', () => {
  const fixtures = acmeTree();
  const place = fixtures.find((row) => row.id === ACME.web)!;
  const thread = fixtures.find((row) => row.id === ACME.mainThread)!;
  const rows = new MeasuredTree();
  for (let index = 0; index < PLACES; index++) rows.set(`place-${index}`, {
    ...place, id: `place-${index}`, parent_id: index === 0 ? null : `place-${index - 1}`,
  });
  const priorities = [...THREAD_URGENCY].reverse();
  for (let index = 0; index < THREADS; index++) rows.set(`thread-${index}`, {
    ...thread, id: `thread-${index}`, parent_id: `place-${PLACES - 1}`,
    thread: { ...thread.thread!, state: 'idle', display: display(priorities[index % priorities.length]!) },
  });
  const state = containedThreadStates(rows);
  expect(state.size).toBe(PLACES);
  expect(state.get('place-0')).toBe('waiting_approval');
  expect(state.get(`place-${PLACES - 1}`)).toBe('waiting_approval');
  // A place needs propagation at most once for each possible urgency improvement, not once per descendant.
  expect(rows.reads).toBeLessThanOrEqual(rows.size * THREAD_URGENCY.length);
  const reads = rows.reads;
  expect(containedThreadStates(rows)).toBe(state);
  expect(rows.reads).toBe(reads);
});

it('terminates a cyclic loaded parent chain while preserving its most urgent authorized state', () => {
  const fixtures = acmeTree();
  const place = fixtures.find((row) => row.id === ACME.web)!;
  const thread = fixtures.find((row) => row.id === ACME.mainThread)!;
  const rows = new MeasuredTree([
    ['one', { ...place, id: 'one', parent_id: 'two' }],
    ['two', { ...place, id: 'two', parent_id: 'one' }],
    ['work', { ...thread, id: 'work', parent_id: 'one', thread: { ...thread.thread!, display: display('held') } }],
  ]);
  expect(containedThreadStates(rows)).toEqual(new Map([['one', 'held'], ['two', 'held']]));
  expect(rows.reads).toBeLessThanOrEqual(rows.size * THREAD_URGENCY.length);
});
