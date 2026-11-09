/** Draft autosave freezes its native destination before an asynchronous mutation can change editors. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useBox, useComposerStore, useDraftSync } from '@/features/composer/Draft';
import type * as Threads from '@/api/threads';
import { id } from '../doubles/fixtures';
import { threadDraft } from '../doubles/thread-rows';
const calls = vi.hoisted(() => ({ save: vi.fn<typeof Threads.saveDraft>() }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), draftQuery: (id: string) => ({ queryKey: ['draft-destination', id], queryFn: async () => null, staleTime: Infinity }), saveDraft: calls.save }));
function Editor({ thread, local }: { thread: string; local: string }) {
  const box = useBox(local); useDraftSync(thread, true, 'queue', true, local);
  return <input aria-label="Private draft" value={box.text} onChange={(event) => box.setText(event.target.value)} />;
}
afterEach(() => { cleanup(); useComposerStore.setState({ text: {}, parts: {} }); vi.useRealTimers(); });
it('keeps the first native thread and private store key while a new editor is selected', async () => {
  vi.useFakeTimers(); calls.save.mockImplementation(async (_id, content) => threadDraft(content));
  let release: () => void = () => undefined;
  const barrier = new Promise<void>((done) => { release = done; });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { onMutate: async () => { await barrier; } } } });
  const first = id(0x75, 1), second = id(0x75, 2);
  const content = (thread: string, local: string) => <QueryClientProvider client={cache}><Editor thread={thread} local={local} /></QueryClientProvider>;
  const view = render(content(first, 'ask:first-editor'));
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Only for the first conversation' } });
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  view.rerender(content(second, 'ask:second-editor'));
  await act(async () => { release(); await vi.advanceTimersByTimeAsync(0); });
  expect(calls.save).toHaveBeenCalledWith(first, [{ type: 'text', text: 'Only for the first conversation' }], 'queue');
  expect(calls.save.mock.calls.some(([thread]) => thread === second)).toBe(false);
  expect(screen.getByRole('textbox')).toHaveValue(''); cache.clear();
});
