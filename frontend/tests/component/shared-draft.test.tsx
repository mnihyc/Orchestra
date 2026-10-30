/** A lane and panel share one draft writer; queued saves cannot restore sent words or cross accounts. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { SessionEvent } from '@/api/client';
import type * as ClientApi from '@/api/client';
import type * as SourceApi from '@/api/source';
import type * as ThreadsApi from '@/api/threads';
import { composerWriter, DRAFT_SAVE_AFTER_MS, submitInput, useBox, useComposerStore, useDraftSync } from '@/features/composer/Draft';
import type { Draft } from '@/generated/api-types';
import { threadWriter } from '@/realtime/stores/thread';
import { threadInput } from '../doubles/thread-rows';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<Draft | null>>(), save: vi.fn<typeof ThreadsApi.saveDraft>(),
  send: vi.fn<typeof ThreadsApi.sendInput>(), clear: vi.fn<() => Promise<void>>(), session: new Set<(event: SessionEvent) => void>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof ClientApi>();
  return { ...actual, client: { ...actual.client, onSession: (listener: (event: SessionEvent) => void) => {
    calls.session.add(listener); return () => { calls.session.delete(listener); };
  } } };
});
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), saveDraft: calls.save, sendInput: calls.send,
  draftQuery: (id: string) => ({ queryKey: ['thread-draft', id], queryFn: calls.read, staleTime: Infinity }) }));
vi.mock('@/api/source', async (load) => ({ ...await load<typeof SourceApi>(), clearDraftText: calls.clear }));

const THREAD = 'shared-side-draft';
const saved: Draft = { content: [], delivery: 'queue', native_action: null, review_comments: [], updated_at: '2026-10-15T18:00:00Z' };
const advance = async (ms: number) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };
function Editor({ name }: { name: string }) {
  const status = useDraftSync(THREAD, true, 'queue');
  return <output aria-label={name}>{useBox(THREAD).text} / {status}</output>;
}
let query: QueryClient;
function showEditors(lane: boolean, panel = true) {
  return <QueryClientProvider client={query}>{lane && <Editor key="lane" name="lane" />}{panel && <Editor key="panel" name="panel" />}</QueryClientProvider>;
}
beforeEach(() => {
  vi.useFakeTimers(); calls.read.mockReset().mockResolvedValue(saved); calls.save.mockReset().mockResolvedValue(saved);
  calls.clear.mockReset().mockResolvedValue(undefined); calls.send.mockReset();
  query = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {} }); threadWriter.evict(THREAD);
});
afterEach(async () => {
  cleanup(); query.clear(); await advance(0); vi.useRealTimers(); threadWriter.evict(THREAD);
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {} });
});

describe('a draft shared by an aside lane and its panel', () => {
  it('flushes the last editor’s dirty words before the debounce when browsing removes the composer', async () => {
    const view = render(showEditors(true, false)); await advance(1);
    act(() => composerWriter.setText(THREAD, 'Keep this while browsing history'));
    expect(calls.save).not.toHaveBeenCalled();
    view.unmount(); await advance(0);
    expect(calls.save).toHaveBeenCalledExactlyOnceWith(THREAD, [{ type: 'text', text: 'Keep this while browsing history' }], 'queue');
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).toHaveBeenCalledTimes(1);
  });

  it('does not flush a retiring editor through another signed-in reader', async () => {
    const view = render(showEditors(true, false)); await advance(1);
    act(() => composerWriter.setText(THREAD, 'Old reader unsaved words'));
    act(() => { for (const listener of calls.session) listener({ type: 'signed_out' }); });
    view.unmount(); await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).not.toHaveBeenCalled(); expect(calls.clear).not.toHaveBeenCalled();
  });

  it('reads and saves once, then lets the remaining editor keep writing when the first closes', async () => {
    const view = render(showEditors(true));
    await advance(1);
    expect(calls.read).toHaveBeenCalledTimes(1);
    act(() => composerWriter.setText(THREAD, 'Which test?'));
    expect(screen.getByRole('status', { name: 'lane' })).toHaveTextContent('Which test?');
    expect(screen.getByRole('status', { name: 'panel' })).toHaveTextContent('Which test?');
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).toHaveBeenCalledExactlyOnceWith(THREAD, [{ type: 'text', text: 'Which test?' }], 'queue');
    view.rerender(showEditors(false));
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).toHaveBeenCalledTimes(1);
    act(() => composerWriter.setText(THREAD, 'Which cookie test?'));
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).toHaveBeenCalledTimes(2);
    expect(calls.save.mock.lastCall).toEqual([THREAD, [{ type: 'text', text: 'Which cookie test?' }], 'queue']);
    expect(calls.read).toHaveBeenCalledTimes(1);
  });

  it('orders the post-send clear after an older save and coalesces the empty editor’s clear', async () => {
    let finishSave: (draft: Draft) => void = () => undefined;
    calls.save.mockImplementation(() => new Promise((resolve) => { finishSave = resolve; }));
    calls.send.mockResolvedValue({ input: threadInput('accepted', 'Which test?', { author: null, position: '1', state: 'delivered' }), seq: 1 });
    render(showEditors(true)); await advance(1);
    act(() => composerWriter.setText(THREAD, 'Which test?')); await advance(DRAFT_SAVE_AFTER_MS);
    act(() => { composerWriter.setText(THREAD, ''); submitInput(THREAD, 'Which test?', 'queue'); });
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.clear).not.toHaveBeenCalled();
    act(() => { finishSave(saved); }); await advance(1);
    expect(calls.clear).toHaveBeenCalledTimes(1);
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.clear).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('status', { name: 'panel' })).not.toHaveTextContent('Which test?');
  });

  it('does not save confirmed words again when a lane is fully replaced by its panel', async () => {
    const lane = render(showEditors(true, false)); await advance(1);
    act(() => composerWriter.setText(THREAD, 'Continue from the panel')); await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).toHaveBeenCalledTimes(1);
    lane.unmount();
    render(showEditors(false, true)); await advance(DRAFT_SAVE_AFTER_MS + 1);
    expect(screen.getByRole('status', { name: 'panel' })).toHaveTextContent('Continue from the panel');
    expect(calls.save).toHaveBeenCalledTimes(1); expect(calls.read).toHaveBeenCalledTimes(1);
  });

  it('never clears newer draft words when an earlier send is acknowledged', async () => {
    let accept: (value: Awaited<ReturnType<typeof ThreadsApi.sendInput>>) => void = () => undefined;
    calls.send.mockImplementation(() => new Promise((resolve) => { accept = resolve; }));
    render(showEditors(true)); await advance(1);
    act(() => { composerWriter.setText(THREAD, ''); submitInput(THREAD, 'The earlier question', 'queue'); composerWriter.setText(THREAD, 'A newer question'); });
    await act(() => Promise.resolve().then(() => { accept({ input: threadInput('accepted-newer', 'The earlier question', { author: null, position: '1' }), seq: 1 }); }));
    await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.clear).not.toHaveBeenCalled();
    expect(calls.save.mock.lastCall).toEqual([THREAD, [{ type: 'text', text: 'A newer question' }], 'queue']);
    expect(screen.getByRole('status', { name: 'panel' })).toHaveTextContent('A newer question');
  });

  it('drops a queued old-account write instead of sending it with the next account’s credentials', async () => {
    let finishSave: (draft: Draft) => void = () => undefined;
    calls.save.mockImplementation(() => new Promise((resolve) => { finishSave = resolve; }));
    const view = render(showEditors(true)); await advance(1);
    act(() => composerWriter.setText(THREAD, 'First account’s words')); await advance(DRAFT_SAVE_AFTER_MS);
    act(() => composerWriter.setText(THREAD, 'A queued old draft')); await advance(DRAFT_SAVE_AFTER_MS);
    expect(calls.save).toHaveBeenCalledTimes(1);
    view.unmount();
    act(() => { for (const listener of calls.session) listener({ type: 'signed_out' }); });
    act(() => { finishSave(saved); }); await advance(1);
    expect(calls.save).toHaveBeenCalledTimes(1); expect(calls.clear).not.toHaveBeenCalled();
  });
});
