/** Moving to the trash: one act for a selection, refusals named without stopping the rest, Undo of exactly that act. */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { organizeDocuments } from '@/api/documentTree';
import { deleteNode, restoreNode } from '@/api/nodes';
import { restoreDocuments, trashDocuments, TREE_ACT_MAX, type TrashEntry } from '@/features/trash/documents';
import { restorePlaces, runningIn, trashPlaces } from '@/features/trash/places';
import { sayTrashed, trashWords } from '@/features/trash/tip';
import { chunks, eachInTurn, outermost } from '@/features/trash/trash';
import type { DocumentTreeOperationBody, DocumentTreeReceipt, Node } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { toast, toastActs } from '@/ui/Toast';

vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), deleteNode: vi.fn(), restoreNode: vi.fn() }));
vi.mock('@/api/documentTree', async (original) => ({ ...await original<object>(), organizeDocuments: vi.fn() }));
vi.mock('@/ui/Toast', async (original) => ({ ...await original<object>(), toast: vi.fn(() => () => undefined) }));

const t = i18n.t.bind(i18n);
const refused = (code = 'authz.denied', status = 403) => new ApiError({ type: `orch:error:${code}`, code, title: 'Refused', detail: 'not allowed', status, request_id: '' }, null);
const place = (id: string, kind: 'topic' | 'thread' | 'project' = 'topic') => ({ id, kind, name: `name-${id}`, parentId: 'parent', archived: false, status: null, folder: false });
const doc = (id: string): TrashEntry => ({ item: { kind: 'document', id }, name: `doc-${id}` });
const body = (call: unknown[]): DocumentTreeOperationBody => call[0] as DocumentTreeOperationBody;

beforeEach(() => { vi.clearAllMocks(); });

describe('one act over a selection', () => {
  it('goes on past a refusal and keeps each outcome with its item', async () => {
    const result = await eachInTurn(['a', 'b', 'c'], (item) => item === 'b' ? Promise.reject(new Error('no')) : Promise.resolve(item.toUpperCase()));
    expect(result.done).toEqual([{ item: 'a', receipt: 'A' }, { item: 'c', receipt: 'C' }]);
    expect(result.failed.map((one) => one.item)).toEqual(['b']);
  });

  it('drops what lies inside another selected thing, and repeats', () => {
    const above: Record<string, string[]> = { child: ['root', 'parent'], parent: ['root'], other: ['root'] };
    expect(outermost(['child', 'parent', 'other', 'parent'], (id) => id, (id) => above[id] ?? [])).toEqual(['parent', 'other']);
  });

  it('splits a selection into runs one act takes', () => {
    expect(chunks([1, 2, 3, 4, 5], 2)).toEqual([[1, 2], [3, 4], [5]]);
  });
});

describe('places', () => {
  it('deletes each place by its own request with no plan, and a refusal does not stop the rest', async () => {
    vi.mocked(deleteNode).mockImplementation((id) => id === 'b' ? Promise.reject(refused()) : Promise.resolve({ deletion_id: `group-${id}` }));
    const result = await trashPlaces([place('a'), place('b'), place('c')]);
    expect(vi.mocked(deleteNode).mock.calls.map((call) => [call[0], call[1]])).toEqual([['a', {}], ['b', {}], ['c', {}]]);
    expect(new Set(vi.mocked(deleteNode).mock.calls.map((call) => call[2])).size).toBe(3);
    expect(result.done.map((one) => one.receipt.deletion_id)).toEqual(['group-a', 'group-c']);
    expect(result.failed.map((one) => one.item.id)).toEqual(['b']);
  });

  it('Undo restores each deletion by its group, the last first, and names what cannot come back', async () => {
    vi.mocked(restoreNode).mockImplementation((id) => id === 'c' ? Promise.reject(refused('conflict.stale_revision', 409)) : Promise.resolve({ id } as unknown as Node));
    const undone = await restorePlaces([
      { item: place('a'), receipt: { deletion_id: 'group-a' } },
      { item: place('side', 'thread'), receipt: { deletion_id: null } },
      { item: place('c'), receipt: { deletion_id: 'group-c' } },
    ], () => 'changed since');
    expect(vi.mocked(restoreNode).mock.calls).toEqual([['c', 'group-c'], ['a', 'group-a']]);
    expect(undone).toEqual({ restored: ['name-a'], refused: [{ name: 'name-c', reason: 'changed since' }] });
  });

  it('counts the running threads in the places and the running threads among them', () => {
    const counts = { orgs: 0, projects: 0, folders: 0, topics: 0, threads: 2, running: 1 };
    expect(runningIn([place('topic'), place('thread', 'thread')], (id) => id === 'topic' ? counts : null, (id) => id === 'thread')).toBe(2);
  });
});

describe('documents', () => {
  const receipt = { operation: 'op', entries: [], copied: [], gaining: 0, losing: 0, restore_until: null } as unknown as DocumentTreeReceipt;

  it('deletes a selection in one act with no plan, and restores it by the same entries', async () => {
    vi.mocked(organizeDocuments).mockResolvedValue(receipt);
    const result = await trashDocuments([doc('a'), doc('b'), doc('a')]);
    expect(organizeDocuments).toHaveBeenCalledTimes(1);
    expect(body(vi.mocked(organizeDocuments).mock.calls[0] ?? [])).toMatchObject({ command: 'delete', items: [{ kind: 'document', id: 'a' }, { kind: 'document', id: 'b' }], deletion_plan: null, to: null });
    expect(result.groups).toEqual([[doc('a'), doc('b')]]);
    const undone = await restoreDocuments(result.groups, () => '');
    expect(body(vi.mocked(organizeDocuments).mock.calls[1] ?? [])).toMatchObject({ command: 'restore', items: [{ kind: 'document', id: 'a' }, { kind: 'document', id: 'b' }] });
    expect(undone).toEqual({ restored: ['doc-a', 'doc-b'], refused: [] });
  });

  it('applies a run refused as a whole entry by entry, so the others still go and the refused one is named', async () => {
    vi.mocked(organizeDocuments).mockImplementation((request) => request.items.length > 1 || request.items[0]?.id === 'b' ? Promise.reject(refused()) : Promise.resolve(receipt));
    const result = await trashDocuments([doc('a'), doc('b'), doc('c')]);
    expect(result.groups).toEqual([[doc('a')], [doc('c')]]);
    expect(result.failed.map((one) => one.item.name)).toEqual(['doc-b']);
  });

  it('does not retry a run whose answer was lost: its entries are named as not moved', async () => {
    vi.mocked(organizeDocuments).mockRejectedValue(new Error('The reply was lost'));
    const result = await trashDocuments([doc('a'), doc('b')]);
    expect(organizeDocuments).toHaveBeenCalledTimes(1);
    expect(result.failed.map((one) => one.item.name)).toEqual(['doc-a', 'doc-b']);
  });

  it('a selection larger than one act takes is several acts', async () => {
    vi.mocked(organizeDocuments).mockResolvedValue(receipt);
    const many = Array.from({ length: TREE_ACT_MAX + 3 }, (_, index) => doc(String(index)));
    const result = await trashDocuments(many);
    expect(vi.mocked(organizeDocuments).mock.calls.map((call) => body(call).items.length)).toEqual([TREE_ACT_MAX, 3]);
    expect(result.groups.flat()).toHaveLength(TREE_ACT_MAX + 3);
  });
});

describe('the one line', () => {
  it('names one thing, counts many, and says a partial result with the refusals', () => {
    expect(trashWords(t, { moved: ['Plan'], refused: [] })).toEqual({ title: 'Moved “Plan” to Trash', failed: false });
    expect(trashWords(t, { moved: ['a', 'b', 'c'], refused: [], stopped: 2 })).toEqual({ title: 'Moved 3 items to Trash', description: '2 running threads stopped', failed: false });
    expect(trashWords(t, { moved: ['a'], refused: [{ name: 'b', reason: 'You may not delete it.' }] })).toEqual({ title: 'Moved 1 of 2 to Trash', description: 'Not moved: b (You may not delete it.)', failed: false });
    expect(trashWords(t, { moved: [], refused: [{ name: 'b', reason: 'You may not delete it.' }] })).toEqual({ title: '“b” was not moved to Trash', description: 'You may not delete it.', failed: true });
  });

  it('offers Show that keeps the line and Undo that reports what came back', async () => {
    const show = vi.fn();
    const undo = vi.fn(() => Promise.resolve({ restored: ['a', 'b'], refused: [] }));
    sayTrashed(t, { moved: ['a', 'b'], refused: [], show, undo });
    const options = vi.mocked(toast).mock.calls[0]?.[0];
    const acts = toastActs(options?.action);
    expect(acts.map((one) => [one.label, one.keep === true])).toEqual([['Show', true], ['Undo', false]]);
    acts[0]?.run();
    expect(show).toHaveBeenCalledOnce();
    acts[1]?.run();
    await vi.waitFor(() => expect(toast).toHaveBeenCalledTimes(2));
    expect(vi.mocked(toast).mock.calls[1]?.[0]).toMatchObject({ tone: 'success', title: '2 items are back' });
  });

  it('offers no acts when nothing moved', () => {
    sayTrashed(t, { moved: [], refused: [{ name: 'a', reason: 'no' }], show: vi.fn(), undo: vi.fn() });
    expect(vi.mocked(toast).mock.calls[0]?.[0]).toMatchObject({ tone: 'danger' });
    expect(toastActs(vi.mocked(toast).mock.calls[0]?.[0].action)).toEqual([]);
  });
});
