/** A quick return to home shows the thread just opened, even while its names answer is still fresh. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import type * as Personal from '@/api/personal';
import { useSeenAfter, useVisitRecorder } from '@/features/navigation/visits';
import type { PersonalTarget } from '@/generated/api-types';
import type * as Screen from '@/shell/screen';

const calls = vi.hoisted(() => ({
  heading: null as { nodeId?: string; target?: PersonalTarget } | null,
  visit: vi.fn<(id: string) => Promise<void>>(), item: vi.fn<(target: PersonalTarget) => Promise<void>>(),
}));
const priorSession = client.session.getState();
const kept = ['home-visit-a', 'home-visit-b', 'home-visit-document'].map((user) => ({ key: `orch.${user}.recents`, value: localStorage.getItem(`orch.${user}.recents`) }));

vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(), recordVisit: calls.visit, recordItemVisit: calls.item }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useCurrentHeading: () => calls.heading }));

beforeEach(() => { for (const entry of kept) localStorage.removeItem(entry.key); });
afterEach(() => {
  cleanup(); client.session.setState(priorSession); calls.heading = null; vi.clearAllMocks(); vi.restoreAllMocks();
  for (const entry of kept) { if (entry.value === null) localStorage.removeItem(entry.key); else localStorage.setItem(entry.key, entry.value); }
});

it('merges only visits newer than the held snapshot, keeps people apart, and remembers each navigation once without a POST', () => {
  calls.visit.mockResolvedValue(undefined); calls.item.mockResolvedValue(undefined);
  client.session.setState({ userId: 'home-visit-a' });
  calls.heading = { nodeId: 'first-thread' };
  const stored = vi.spyOn(Storage.prototype, 'setItem');
  const hook = renderHook(({ snapshotAt }) => { useVisitRecorder(); return useSeenAfter(snapshotAt); }, { initialProps: { snapshotAt: 1 } });
  expect(hook.result.current).toEqual(['first-thread']);
  expect(stored).toHaveBeenCalledExactlyOnceWith('orch.home-visit-a.recents', JSON.stringify(['first-thread']));
  calls.heading = { nodeId: 'second-thread' };
  hook.rerender({ snapshotAt: 1 });
  expect(hook.result.current).toEqual(['second-thread', 'first-thread']);
  expect(stored).toHaveBeenCalledTimes(2);
  expect(localStorage.getItem('orch.home-visit-a.recents')).toBe(JSON.stringify(['second-thread', 'first-thread']));
  hook.rerender({ snapshotAt: 1 });
  expect(stored).toHaveBeenCalledTimes(2);
  hook.rerender({ snapshotAt: Number.MAX_SAFE_INTEGER });
  expect(hook.result.current).toEqual([]);
  calls.heading = null;
  act(() => client.session.setState({ userId: 'home-visit-b' }));
  expect(hook.result.current).toEqual([]);
  expect(localStorage.getItem('orch.home-visit-b.recents')).toBeNull();
  expect(stored).toHaveBeenCalledTimes(2);
  expect(calls.visit).not.toHaveBeenCalled();
  expect(calls.item).not.toHaveBeenCalled();
});

it('remembers a canonically addressed node locally without posting another server visit', () => {
  calls.visit.mockResolvedValue(undefined); calls.item.mockResolvedValue(undefined);
  client.session.setState({ userId: 'home-visit-b' });
  calls.heading = { nodeId: 'addressed-thread', target: { kind: 'thread', id: 'addressed-thread' } };
  const hook = renderHook(() => { useVisitRecorder(); return useSeenAfter(1); });
  expect(hook.result.current).toEqual(['addressed-thread']);
  expect(localStorage.getItem('orch.home-visit-b.recents')).toBe(JSON.stringify(['addressed-thread']));
  expect(calls.item).not.toHaveBeenCalled();
  expect(calls.visit).not.toHaveBeenCalled();
});

it('a document visit never pretends that its containing thread was opened', () => {
  calls.visit.mockResolvedValue(undefined); calls.item.mockResolvedValue(undefined);
  client.session.setState({ userId: 'home-visit-document' });
  calls.heading = { nodeId: 'beside-thread', target: { kind: 'document', id: 'a-document' } };
  const hook = renderHook(() => { useVisitRecorder(); return useSeenAfter(1); });
  expect(hook.result.current).toEqual([]);
  expect(localStorage.getItem('orch.home-visit-document.recents')).toBeNull();
  expect(calls.item).not.toHaveBeenCalled();
  expect(calls.visit).not.toHaveBeenCalled();
});
