/** Unanswered side acts survive navigation and retain their body; acceptance cannot erase newer work. */
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as ForksApi from '@/api/forks';
import type * as ThreadsApi from '@/api/threads';
import type * as SourceApi from '@/api/source';
import { draftQuery } from '@/api/threads';
import { composerWriter, useAttachmentParts, useBox, useComposerStore, useDraftSync } from '@/features/composer/Draft';
import { forkKeys } from '@/api/forks';
import { sideLaneKey, timelineUi, useTimelineUi } from '@/features/timeline/state';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { asideUnconfirmed, useAskOnSide, useSideAsk } from '@/features/side-threads/ask';
import type { Draft, Thread } from '@/generated/api-types';
import { threadInput } from '../doubles/thread-rows';

const calls = vi.hoisted(() => ({ start: vi.fn<typeof ForksApi.startSide>(), accepted: vi.fn<(text: string) => void>(), draft: vi.fn<() => Promise<Draft | null>>(), clear: vi.fn<() => Promise<void>>() }));
vi.mock('@/api/forks', async (original) => ({ ...await original<typeof ForksApi>(), startSide: calls.start }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof ThreadsApi>(), draftQuery: (id: string) => ({ queryKey: ['aside-draft', id], queryFn: calls.draft, staleTime: Infinity }) }));
vi.mock('@/api/source', async (original) => ({ ...await original<typeof SourceApi>(), clearDraftText: calls.clear }));
const THREAD = 'aside-parent';
type Reply = Awaited<ReturnType<typeof ForksApi.startSide>>;
const reply = (): Reply => ({ thread: { id: 'accepted-side', kind: 'thread', name: 'Parent (side)', seq: 1, revision: 1,
  side_of: { id: THREAD, kind: 'thread', name: 'Parent', redacted: false } } as Thread,
  input: { ...threadInput('accepted', 'The original question', { author: null, position: '1' }), thread_id: 'accepted-side', seq_updated: 1 }, seq: 1 });

function mount(toTurn: number | null = null, query = new QueryClient({ defaultOptions: { mutations: { retry: false } } })) {
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={query}>{children}</QueryClientProvider>;
  return renderHook(() => useAskOnSide(THREAD, { toTurn, onAccepted: (_reply, question) => {
    calls.accepted(question.text);
    composerWriter.clearAcceptedAside(THREAD, question.text);
  } }), { wrapper });
}

beforeEach(() => { calls.start.mockReset(); calls.accepted.mockClear(); calls.draft.mockReset(); calls.clear.mockReset().mockResolvedValue(undefined); useSideAsk.getState().forget(); useComposerStore.setState({ text: {}, parts: {} }); });
afterEach(() => { cleanup(); useSideAsk.getState().forget(); useComposerStore.setState({ text: {}, parts: {} }); threadWriter.evict('accepted-side'); timelineUi.forget(THREAD); });

describe('an aside whose answer may have been lost', () => {
  it.each([false, true])('projects acceptance into its side and a complete held list only (list loaded: %s)', async (loaded) => {
    const query = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    if (loaded) query.setQueryData(forkKeys.sides(THREAD, 'open'), [{ id: 'an-older-aside' }]);
    calls.start.mockResolvedValue(reply());
    const view = mount(null, query);
    act(() => { view.result.current.mutate({ text: 'The original question', toTurn: null }); });
    await waitFor(() => expect(calls.accepted).toHaveBeenCalledTimes(1));
    expect(threadViewOf('accepted-side').thread?.id).toBe('accepted-side');
    expect(threadViewOf('accepted-side').inputs.has('accepted')).toBe(true);
    const ui = useTimelineUi.getState().byThread.get(THREAD);
    expect(ui?.steps.get(sideLaneKey('accepted-side'))).toBe(true);
    expect(ui?.sideFocus?.id).toBe('accepted-side');
    const listed = query.getQueryData<Thread[]>(forkKeys.sides(THREAD, 'open'));
    if (loaded) expect(listed?.map((side) => side.id)).toEqual(['an-older-aside', 'accepted-side']);
    else expect(listed).toBeUndefined();
    view.unmount(); query.clear();
  });
  const savedDraft: Draft = { content: [{ type: 'text', text: 'The original question' }], delivery: 'queue', native_action: null, review_comments: [], updated_at: '2026-10-15T12:00:00Z' };
  function restoreComposer() {
    const query = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={query}>{children}</QueryClientProvider>;
    return renderHook(() => {
      const loaded = useQuery(draftQuery(THREAD));
      useDraftSync(THREAD, true, 'queue');
      return { ready: loaded.isSuccess, text: useBox(THREAD).text, parts: useAttachmentParts(THREAD) };
    }, { wrapper });
  }

  it('still restores a server draft when this page has no local draft yet', async () => {
    calls.draft.mockResolvedValue(savedDraft);
    const composer = restoreComposer();
    await waitFor(() => expect(composer.result.current.ready).toBe(true));
    expect(composer.result.current.text).toBe('The original question');
    expect(calls.draft).toHaveBeenCalledTimes(1);
  });

  it('does not restore an acknowledged message or intentionally removed files from an older server answer', async () => {
    calls.draft.mockResolvedValue({ ...savedDraft, content: [...savedDraft.content, { type: 'image', blob: 'old-file', name: 'old.png', media_type: 'image/png' }] });
    composerWriter.setText(THREAD, 'The original question');
    composerWriter.clearAcceptedAside(THREAD, 'The original question');
    composerWriter.setParts(THREAD, []);
    const composer = restoreComposer();
    await waitFor(() => expect(composer.result.current.ready).toBe(true));
    expect(composer.result.current.text).toBe('');
    expect(composer.result.current.parts).toEqual([]);
    expect(calls.draft).toHaveBeenCalledTimes(1);
  });

  it('keeps its frozen body and idempotency value through navigation, then preserves newer words on acceptance', async () => {
    calls.start.mockRejectedValueOnce(new Error('The answer did not arrive.'));
    let accept: (value: Reply) => void = () => undefined;
    calls.start.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    composerWriter.setText(THREAD, 'The original question');
    const first = mount(2);
    act(() => { first.result.current.mutate({ text: 'The original question', toTurn: 2 }); });
    await waitFor(() => expect(first.result.current.unconfirmed).toBe(true));
    const original = calls.start.mock.calls[0];
    first.unmount();
    composerWriter.setText(THREAD, 'A newer question I am still writing');
    const again = mount(2);
    expect(again.result.current.unconfirmed).toBe(true);
    expect(again.result.current.variables).toEqual({ text: 'The original question', toTurn: 2 });
    // Even a stale caller cannot bind newer words to the old idempotency value.
    act(() => { again.result.current.mutate({ text: 'A newer question I am still writing', toTurn: 2 }); });
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(2));
    expect(calls.start.mock.calls[1]).toEqual(original);
    expect(again.result.current.unconfirmed).toBe(true);
    expect(again.result.current.isPending).toBe(true);
    await act(() => Promise.resolve().then(() => { accept(reply()); }));
    await waitFor(() => expect(again.result.current.isPending).toBe(false));
    expect(again.result.current.unconfirmed).toBe(false);
    expect(calls.accepted).toHaveBeenCalledExactlyOnceWith('The original question');
    expect(useComposerStore.getState().text[THREAD]).toBe('A newer question I am still writing');
  });

  it('coalesces simultaneous consumers of one history boundary into one native act', async () => {
    let accept: (value: Reply) => void = () => undefined;
    calls.start.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    const first = mount(), second = mount();
    act(() => {
      first.result.current.mutate({ text: 'The original question', toTurn: null });
      second.result.current.mutate({ text: 'A duplicate press with different text', toTurn: null });
    });
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
    expect(first.result.current.isPending).toBe(true); expect(second.result.current.isPending).toBe(true);
    expect(calls.start.mock.calls[0]?.[1].content).toEqual([{ type: 'text', text: 'The original question' }]);
    await act(() => Promise.resolve().then(() => { accept(reply()); }));
    await waitFor(() => expect(second.result.current.isPending).toBe(false));
    expect(calls.start).toHaveBeenCalledTimes(1);
  });

  it('clears an unchanged accepted draft even after its composer has unmounted', async () => {
    let accept: (value: Reply) => void = () => undefined;
    calls.start.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    composerWriter.setText(THREAD, 'The original question');
    const view = mount();
    act(() => { view.result.current.mutate({ text: 'The original question', toTurn: null }); });
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
    view.unmount();
    await act(() => Promise.resolve().then(() => { accept(reply()); }));
    await waitFor(() => expect(calls.accepted).toHaveBeenCalledExactlyOnceWith('The original question'));
    expect(useComposerStore.getState().text[THREAD]).toBe('');
  });

  it('keeps new attachments and their unchanged accompanying words when an earlier aside is accepted', async () => {
    let accept: (value: Reply) => void = () => undefined;
    calls.start.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    composerWriter.setText(THREAD, 'The original question');
    const view = mount();
    act(() => { view.result.current.mutate({ text: 'The original question', toTurn: null }); });
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
    const image = { type: 'image', blob: 'new-image', name: 'new.png', media_type: 'image/png' } as const;
    composerWriter.setParts(THREAD, [image]);
    await act(() => Promise.resolve().then(() => { accept(reply()); }));
    await waitFor(() => expect(calls.accepted).toHaveBeenCalledTimes(1));
    expect(useComposerStore.getState().parts[THREAD]).toEqual([image]);
    expect(useComposerStore.getState().text[THREAD]).toBe('The original question');
  });

  it('lets a known refusal be corrected with a new act, rather than freezing it as an unknown send', async () => {
    calls.start.mockRejectedValueOnce(new ApiError({ type: 'about:blank', title: 'Refused', status: 403, code: 'authz.denied', detail: 'This thread is no longer shared.', request_id: 'aside-refused' }, null));
    calls.start.mockResolvedValue(reply());
    const view = mount();
    act(() => { view.result.current.mutate({ text: 'The original question', toTurn: null }); });
    await waitFor(() => expect(view.result.current.error).toBeInstanceOf(ApiError));
    expect(view.result.current.unconfirmed).toBe(false);
    const firstId = calls.start.mock.calls[0]?.[1].client_id;
    act(() => { view.result.current.mutate({ text: 'The corrected question', toTurn: null }); });
    await waitFor(() => expect(calls.accepted).toHaveBeenCalledExactlyOnceWith('The corrected question'));
    expect(calls.start.mock.calls[1]?.[1].client_id).not.toBe(firstId);
    expect(calls.start.mock.calls[1]?.[1].content).toEqual([{ type: 'text', text: 'The corrected question' }]);
  });

  it('forgets attempts on an account lifetime change and ignores an old acceptance for the next person', async () => {
    let accept: (value: Reply) => void = () => undefined;
    calls.start.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    const old = mount();
    act(() => { old.result.current.mutate({ text: 'The original question', toTurn: null }); });
    await waitFor(() => expect(calls.start).toHaveBeenCalledTimes(1));
    old.unmount();
    useSideAsk.getState().forget();
    composerWriter.setText(THREAD, 'The original question');
    const next = mount();
    expect(next.result.current.unconfirmed).toBe(false);
    expect(next.result.current.variables).toBeUndefined();
    await act(() => Promise.resolve().then(() => { accept(reply()); }));
    expect(calls.accepted).not.toHaveBeenCalled();
    expect(useComposerStore.getState().text[THREAD]).toBe('The original question');
    expect(asideUnconfirmed(null)).toBe(false); expect(asideUnconfirmed(undefined)).toBe(false);
  });
});
