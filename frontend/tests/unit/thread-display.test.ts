import { threadIconFacts } from '@/ui/ThreadState';
/** Personal read updates cannot be overtaken by delayed thread or tree icon frames. */
import { afterEach, expect, it } from 'vitest';
import { containedThreadStates, currentThreadDisplay, isThreadDisplay } from '@/lib/threadDisplay';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { threadWriter, threadViewOf, viewFromRows } from '@/realtime/stores/thread';
import { buildRows } from '@/features/navigation/treeRows';
import type { Thread, TreeNode } from '@/generated/api-types';
import { display, recoveryDisplay } from '../doubles/thread-display';
import { acmeTree, ACME } from '../doubles/fixtures';

afterEach(() => { treeWriter.clear(); threadWriter.evict(ACME.mainThread); });

it('keeps a read acknowledgement ahead of an earlier frame and accepts the next completed turn', () => {
  const old = display('finished', { seq: 20, read_seq: 10 });
  const seen = display('idle', { seq: 20, read_seq: 20 });
  const next = display('finished', { seq: 30, read_seq: 20 });
  expect(currentThreadDisplay(old, seen)).toBe(seen);
  expect(currentThreadDisplay(seen, old)).toBe(seen);
  expect(currentThreadDisplay(next, seen)).toBe(next);
  expect(isThreadDisplay({ ...seen, state: 'starting' })).toBe(false);
  expect(isThreadDisplay({ ...seen, waiting_for_answer: 'yes' })).toBe(false);
  expect(isThreadDisplay(display('queued', { recovery: recoveryDisplay({ attempt: 4_294_967_295, attempts: 4_294_967_295 }) }))).toBe(true);
  expect(isThreadDisplay({ ...seen, recovery: { ...recoveryDisplay(), attempt: '2' } })).toBe(false);
  expect(isThreadDisplay({ ...seen, recovery: { ...recoveryDisplay(), stop_reason: 'private vendor text' } })).toBe(false);
});

it('updates existing tree rows and open windows without acquiring another read', () => {
  const row = acmeTree().find((node) => node.id === ACME.mainThread)!;
  const old = display('finished', { seq: 20, read_seq: 10 });
  const seen = display('idle', { seq: 20, read_seq: 20 });
  treeWriter.reset(ACME.org, [{ ...row, thread: { ...row.thread!, display: old } }], 1);
  threadWriter.reset(row.id, { thread: { ...row, display: old, seq: 20, revision: 1 } as unknown as Thread }, false);
  treeWriter.display(row.id, seen); threadWriter.display(row.id, seen);
  expect(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(row.id)?.thread?.display).toBe(seen);
  expect(threadViewOf(row.id).thread?.display).toBe(seen);
  treeWriter.apply(ACME.org, [{ ...row, tree_seq: 2, thread: { ...row.thread!, display: old } }], 2);
  expect(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(row.id)?.thread?.display).toBe(seen);
  treeWriter.reset(ACME.org, [{ ...row, thread: { ...row.thread!, display: old } }], 2);
  expect(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(row.id)?.thread?.display).toBe(seen);
  threadWriter.reset(row.id, { thread: { ...row, display: old, seq: 20, revision: 1 } as unknown as Thread }, false);
  expect(threadViewOf(row.id).thread?.display).toBe(seen);
  const another = viewFromRows(threadViewOf(row.id), { thread: { ...row, id: 'another-thread', display: old, seq: 20, revision: 1 } as unknown as Thread }, false);
  expect(another.thread?.display).toBe(old);
});

it('places expose the same urgency with decisions before blockers and new results before quiet work', () => {
  const rows = acmeTree();
  const first = rows.find((node) => node.id === ACME.mainThread)!;
  const parent = first.parent_id!;
  const peers: TreeNode[] = ['blocked', 'waiting_approval', 'finished'].map((state, index) => ({ ...first, id: `peer-${index}`, thread: { ...first.thread!, state: 'idle', display: display(state as 'blocked' | 'waiting_approval' | 'finished') } }));
  const nodes = new Map(rows.filter((node) => node.kind !== 'thread').map((node) => [node.id, node]));
  for (const node of peers) nodes.set(node.id, node);
  const tree = { nodes, lastSeq: 1, loaded: true, error: null };
  const made = buildRows(tree, ACME.org, new Set([ACME.research, ACME.web]), 'en');
  const topic = made.find((row) => row.id === parent);
  expect(topic?.item.type === 'node' ? topic.item.urgent : null).toBe('waiting_approval');
});

it('keeps cached identity readable while a new presentation has not arrived yet', () => {
  expect(threadIconFacts({ id: 'cached' })).toBeUndefined();
  expect(threadIconFacts({ id: 'current', display: display('held') })?.display.state).toBe('held');
  const row = acmeTree().find((node) => node.id === ACME.mainThread)!;
  const cached = { ...row, thread: { ...row.thread, display: undefined } } as unknown as TreeNode;
  expect(containedThreadStates(new Map([[cached.id, cached]])).size).toBe(0);
});
