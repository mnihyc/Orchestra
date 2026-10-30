/** Approval relationships cannot replace the complete thread shown by its page or menus. */
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { applyRow, applyThread, isCompleteThread, type ThreadChange } from '@/api/cache';
import type { ApprovalDecisionReply, Thread } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

function nativeThread(): Thread {
  const world = new World();
  const person = world.people.get('ada');
  const row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('the native world supplies Ada and her thread');
  return world.threads.thread(person, row);
}

it.each(['Allow once', 'Send answers'])('%s leaves the complete thread and its visible export control intact', async () => {
  const thread = nativeThread();
  const cache = new QueryClient();
  cache.setQueryData(['node', thread.id], thread);
  const read = vi.fn(() => Promise.resolve(thread));
  function Page() {
    const query = useQuery({ queryKey: ['node', thread.id], queryFn: read, staleTime: Infinity });
    return <main><h1>{query.data?.name}</h1><button disabled={query.data?.can.export !== true}>Export thread</button></main>;
  }
  const view = render(<QueryClientProvider client={cache}><Page /></QueryClientProvider>);
  try {
    const answer: Pick<ApprovalDecisionReply, 'id' | 'kind' | 'state' | 'node' | 'thread_id'> = {
      id: 'approval-1', kind: 'harness', state: 'approved', thread_id: thread.id,
      node: { id: thread.id, kind: 'thread', name: thread.name, redacted: false },
    };
    await act(async () => {
      await applyChanges(findAct('POST', '/approvals/approval-1/decision', { answer })?.changes ?? [], cache);
    });
    expect(cache.getQueryData(['node', thread.id])).toEqual(thread);
    expect(cache.getQueryData(['approval', answer.id])).toMatchObject({ state: 'approved' });
    await waitFor(() => expect(screen.getByRole('heading', { name: thread.name })).toBeVisible());
    expect(screen.getByRole('button', { name: 'Export thread' })).toHaveProperty('disabled', !thread.can.export);
    expect(read).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});

it('allows only a complete native thread through the replacement path, including the next live state', () => {
  const thread = nativeThread();
  const partialCannotReplace: Partial<Thread> extends ThreadChange['thread'] ? true : false = false;
  expect(partialCannotReplace).toBe(false);
  expect(isCompleteThread(thread)).toBe(true);
  const cache = new QueryClient();
  cache.setQueryData(['node', thread.id], thread);
  applyRow(cache, { kind: 'node', id: thread.id, value: { id: thread.id, kind: 'thread', name: 'Renamed', access: thread.access } });
  expect(cache.getQueryData(['node', thread.id])).toEqual({ ...thread, name: 'Renamed' });
  applyRow(cache, { kind: 'thread', id: thread.id, value: { id: thread.id, can: null, binding: null }, partial: true });
  expect(cache.getQueryData(['node', thread.id])).toEqual({ ...thread, name: 'Renamed' });
  const next: Thread = { ...thread, seq: thread.seq + 1, name: 'Renamed', state: 'idle', pending_approvals: 0 };
  applyThread(cache, next);
  expect(cache.getQueryData(['node', thread.id])).toEqual(next);
  expect(isCompleteThread({ ...next, can: undefined })).toBe(false);
  expect(isCompleteThread({ ...next, input_access: undefined })).toBe(false);
  cache.clear();
});
