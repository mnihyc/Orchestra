/** A hidden composer reads no draft; two shown copies share one native draft key and never save merely on opening. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Chats from '@/api/chats';
import { SAVE_AFTER_MS, useDraft, useChatDraftGuard } from '@/features/chat/draft';
import { draftKey } from '@/api/chats';
import { show } from './support';

const reads = vi.hoisted(() => ({ draft: vi.fn<typeof Chats.fetchDraft>(), save: vi.fn<typeof Chats.saveDraft>(), clear: vi.fn<typeof Chats.discardDraft>() }));
vi.mock('@/api/chats', async (load) => ({ ...await load<typeof Chats>(), fetchDraft: reads.draft, saveDraft: reads.save, discardDraft: reads.clear }));
function Editors({ shown }: { shown: boolean }) {
  const first = useDraft('shared-chat-draft-read', shown), second = useDraft('shared-chat-draft-read', shown);
  return <><output aria-label="First draft">{first.text}</output><output aria-label="Second draft">{second.text}</output></>;
}
afterEach(() => { vi.clearAllMocks(); vi.useRealTimers(); });

it('reads once on showing both composers, never in a hidden minute or in response to invalidating their hidden cache', async () => {
  reads.draft.mockResolvedValue({ parts: [{ type: 'text', text: 'The kept team message' }], reply_to: null, mention_names: [], updated_at: '2026-10-16T00:00:00Z' });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const draw = (shown: boolean) => <QueryClientProvider client={cache}><Editors shown={shown} /></QueryClientProvider>;
  const view = show(draw(false));
  try {
    await act(async () => { await Promise.resolve(); });
    expect(reads.draft).not.toHaveBeenCalled();
    view.rerender(draw(true));
    expect(await screen.findAllByText('The kept team message')).toHaveLength(2);
    expect(reads.draft).toHaveBeenCalledExactlyOnceWith('shared-chat-draft-read');
    view.rerender(draw(false));
    vi.useFakeTimers();
    await act(async () => { await cache.invalidateQueries(); await vi.advanceTimersByTimeAsync(60_000); });
    expect(reads.draft).toHaveBeenCalledTimes(1);
    expect(reads.save).not.toHaveBeenCalled(); expect(reads.clear).not.toHaveBeenCalled();
    expect(screen.getByLabelText('First draft')).toHaveTextContent('The kept team message');
    expect(screen.getByLabelText('Second draft')).toHaveTextContent('The kept team message');
  } finally { view.unmount(); cache.clear(); }
});

it('keeps the confirmed words when its panel moves, without reading or saving the same draft again', async () => {
  reads.draft.mockResolvedValue(null);
  reads.save.mockImplementation((_id, body) => Promise.resolve({ parts: body.parts, reply_to: body.reply_to ?? null, mention_names: [], updated_at: '2026-10-16T00:00:00Z' }));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const id = 'chat-draft-moved-after-ack'; cache.setQueryData(draftKey(id), null);
  function Editor() {
    const draft = useDraft(id, true);
    return <input aria-label="Moved draft" value={draft.text} onChange={(event) => draft.setText(event.target.value)} />;
  }
  const draw = (place: string) => <QueryClientProvider client={cache}><Editor key={place} /></QueryClientProvider>;
  const view = show(draw('panel'));
  vi.useFakeTimers();
  try {
    fireEvent.change(screen.getByLabelText('Moved draft'), { target: { value: 'Keep these confirmed words' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(reads.save).toHaveBeenCalledTimes(1);
    view.rerender(draw('expanded'));
    expect(screen.getByLabelText('Moved draft')).toHaveValue('Keep these confirmed words');
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS * 2); });
    expect(reads.draft).not.toHaveBeenCalled(); expect(reads.save).toHaveBeenCalledTimes(1); expect(reads.clear).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});

it('protects a failed draft behind another view and retries the exact words without a new draft read', async () => {
  const id = 'failed-draft-recovery';
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(draftKey(id), null);
  reads.save.mockRejectedValue(new Error('Offline'));
  function Editor() {
    const draft = useDraft(id, true);
    return <><input aria-label="Recoverable draft" value={draft.text} onChange={(event) => draft.setText(event.target.value)} /><output aria-label="Save state">{draft.saving}</output><button onClick={draft.retry}>Retry draft</button></>;
  }
  function Frame({ shown }: { shown: boolean }) {
    useChatDraftGuard();
    return <QueryClientProvider client={cache}>{shown && <Editor />}</QueryClientProvider>;
  }
  const warned = () => { const event = new Event('beforeunload', { cancelable: true }); window.dispatchEvent(event); return event.defaultPrevented; };
  const view = show(<Frame shown />);
  vi.useFakeTimers();
  try {
    fireEvent.change(screen.getByLabelText('Recoverable draft'), { target: { value: 'Please preserve this unsent report.' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(screen.getByLabelText('Save state')).toHaveTextContent('failed');
    expect(warned()).toBe(true);
    view.rerender(<Frame shown={false} />);
    await act(async () => { await Promise.resolve(); });
    const calls = reads.save.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(reads.save).toHaveBeenCalledTimes(calls);
    expect(warned()).toBe(true);
    view.rerender(<Frame shown />);
    expect(screen.getByLabelText('Recoverable draft')).toHaveValue('Please preserve this unsent report.');
    reads.save.mockImplementation((_chat, body) => Promise.resolve({ ...body, reply_to: body.reply_to ?? null, mention_names: [], updated_at: '2026-10-16T00:00:00Z' }));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Retry draft' })); await Promise.resolve(); });
    expect(reads.save).toHaveBeenLastCalledWith(id, { parts: [{ type: 'text', text: 'Please preserve this unsent report.' }], reply_to: null });
    expect(screen.getByLabelText('Save state')).toHaveTextContent('saved');
    expect(warned()).toBe(false);
    expect(reads.draft).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});

it('serializes a retired composer save before newer words and keeps their warning after failure', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const id = 'overlapping-chat-save'; cache.setQueryData(draftKey(id), null);
  let finish: ((reply: Awaited<ReturnType<typeof Chats.saveDraft>>) => void) | undefined;
  reads.save.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; })).mockRejectedValue(new Error('Newer save refused'));
  function Editor() {
    const draft = useDraft(id, true);
    return <><input aria-label="Current draft" value={draft.text} onChange={(event) => draft.setText(event.target.value)} /><output aria-label="Current save">{draft.saving}</output><button onClick={draft.retry}>Retry current</button></>;
  }
  function Frame({ placement }: { placement: string }) { useChatDraftGuard(); return <QueryClientProvider client={cache}><Editor key={placement} /></QueryClientProvider>; }
  const warned = () => { const event = new Event('beforeunload', { cancelable: true }); window.dispatchEvent(event); return event.defaultPrevented; };
  const view = show(<Frame placement="panel" />); vi.useFakeTimers();
  try {
    fireEvent.change(screen.getByLabelText('Current draft'), { target: { value: 'Older words' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(reads.save).toHaveBeenCalledTimes(1);
    view.rerender(<Frame placement="expanded" />);
    fireEvent.change(screen.getByLabelText('Current draft'), { target: { value: 'Newer unsaved words' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(SAVE_AFTER_MS); });
    expect(reads.save).toHaveBeenCalledTimes(1);
    await act(async () => { finish?.({ parts: [{ type: 'text', text: 'Older words' }], reply_to: null, mention_names: [], updated_at: '2026-10-16T00:00:00Z' }); await Promise.resolve(); });
    expect(screen.getByLabelText('Current draft')).toHaveValue('Newer unsaved words');
    expect(screen.getByLabelText('Current save')).toHaveTextContent('failed');
    expect(warned()).toBe(true);
    expect(reads.save.mock.calls.map(([, body]) => body.parts)).toEqual([[{ type: 'text', text: 'Older words' }], [{ type: 'text', text: 'Newer unsaved words' }]]);
    reads.save.mockImplementation((_chat, body) => Promise.resolve({ ...body, reply_to: body.reply_to ?? null, mention_names: [], updated_at: '2026-10-16T00:00:01Z' }));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Retry current' })); await Promise.resolve(); });
    expect(warned()).toBe(false);
    expect(screen.getByLabelText('Current save')).toHaveTextContent('saved');
  } finally { view.unmount(); cache.clear(); }
});
