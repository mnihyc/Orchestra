/** One visible Ask opening owns its snapshot, ordered events and per-conversation presentation. */
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { useLayoutEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AskApi from '@/api/ask';
import { askKey } from '@/api/ask';
import { queryClient } from '@/api/query';
import { useConversation } from '@/features/ask/useConversation';
import type { AskEvent, AskGetReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ASK_ANSWER, ASK_ID, askConversation, askMessage, askSnapshot } from '../unit/ask-fixtures';

interface Delivery { id: string; after: number; receive: (event: AskEvent) => void; signal: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(id: string) => Promise<AskGetReply>>(), stream: vi.fn<typeof AskApi.streamAsk>(), deliveries: [] as Delivery[] }));
vi.mock('@/api/ask', async (original) => {
  const actual = await original<typeof AskApi>();
  return { ...actual, askQuery: (id: string) => ({ queryKey: actual.askKey(id), queryFn: () => calls.read(id), retry: false, staleTime: 0 }), streamAsk: calls.stream };
});
const frames: { id: string; notice: string; deleted: boolean }[] = [];

function Probe({ id, visible }: { id: string; visible: boolean }) {
  const state = useConversation(id, visible);
  useLayoutEffect(() => { frames.push({ id, notice: state.notice, deleted: state.deleted }); });
  return <><div data-testid="answer">{state.data?.messages.flatMap((message) => message.content.flatMap((part) => part.type === 'text' ? [part.text] : [])).join(' ') ?? 'No snapshot'}</div>
    <div data-testid="notice">{state.notice}</div><div data-testid="tools">{Object.values(state.tools).join(' ')}</div><div data-testid="deleted">{String(state.deleted)}</div></>;
}
const probe = (id = ASK_ID, visible = true) => <QueryClientProvider client={queryClient}><Probe id={id} visible={visible} /></QueryClientProvider>;
const deliver = async (data: AskEvent['data'], id = ASK_ID, seq = 6) => {
  const stream = calls.deliveries.at(-1); if (stream === undefined) throw new Error('The visible view owns a stream');
  await act(async () => { stream.receive({ conversation_id: id, seq, data }); await Promise.resolve(); });
};

beforeEach(() => {
  queryClient.clear(); frames.length = 0; calls.deliveries.length = 0; calls.read.mockReset(); calls.stream.mockReset();
  calls.read.mockImplementation((id) => Promise.resolve(askSnapshot({ conversation: askConversation({ id, state: 'running' }) })));
  calls.stream.mockImplementation((id, after, receive, signal) => {
    calls.deliveries.push({ id, after, receive, signal });
    return new Promise<void>((resolve) => { if (signal.aborted) resolve(); else signal.addEventListener('abort', () => resolve(), { once: true }); });
  });
});
afterEach(async () => { cleanup(); queryClient.clear(); await setLocale('en'); });

it('does not read while hidden and closes its event reader when hidden again', async () => {
  const view = render(probe(ASK_ID, false)); expect(calls.read).not.toHaveBeenCalled(); expect(calls.stream).not.toHaveBeenCalled();
  view.rerender(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  expect(calls.deliveries[0]?.after).toBe(5);
  view.rerender(probe(ASK_ID, false)); await waitFor(() => expect(calls.deliveries[0]?.signal.aborted).toBe(true));
});

it('ignores foreign notices, tools and deletion before they affect the visible conversation', async () => {
  render(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  const other = '00000000-0000-4000-8000-000000000099';
  await deliver({ kind: 'notice', code: 'fixture', message: 'Another organization notice' }, other);
  await deliver({ kind: 'tool', call_id: 'foreign', name: 'foreign source', complete: false, citations: [] }, other, 7);
  await deliver({ kind: 'deleted' }, other, 8);
  expect(screen.getByTestId('notice')).toBeEmptyDOMElement(); expect(screen.getByTestId('tools')).toBeEmptyDOMElement();
  expect(screen.getByTestId('deleted')).toHaveTextContent('false'); expect(screen.getByTestId('answer')).toHaveTextContent('Read the release notes.');
  expect(queryClient.getQueryData<AskGetReply>(askKey(ASK_ID))?.conversation.seq).toBe(5);
});

it('retains ordered unseen notices when a native snapshot overtakes the active event reader', async () => {
  render(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  const newer = askSnapshot({ conversation: askConversation({ state: 'ready', seq: 9 }) });
  act(() => { queryClient.setQueryData(askKey(ASK_ID), newer); });
  await deliver({ kind: 'error', code: 'ask_context_no_room', message: 'Server fallback' }, ASK_ID, 7);
  expect(screen.getByTestId('notice')).toHaveTextContent(i18n.t('ask.contextNoRoom'));
  expect(queryClient.getQueryData(askKey(ASK_ID))).toEqual(newer);
  await deliver({ kind: 'notice', code: 'old', message: 'A duplicate must not replace the reason' }, ASK_ID, 7);
  await deliver({ kind: 'notice', code: 'foreign', message: 'A foreign notice must not replace the reason' }, 'another-conversation', 8);
  await deliver({ kind: 'tool', call_id: 'settled', name: 'A stale tool must not return', complete: false, citations: [] }, ASK_ID, 8);
  expect(screen.getByTestId('notice')).toHaveTextContent(i18n.t('ask.contextNoRoom'));
  expect(screen.getByTestId('tools')).toBeEmptyDOMElement();
  expect(queryClient.getQueryData(askKey(ASK_ID))).toEqual(newer);
});

it('does not render a former conversation’s notice during even the first frame of a new opening', async () => {
  const view = render(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  await deliver({ kind: 'notice', code: 'fixture', message: 'A former conversation notice' });
  expect(screen.getByTestId('notice')).toHaveTextContent('A former conversation notice');
  const other = '00000000-0000-4000-8000-000000000099'; view.rerender(probe(other));
  await waitFor(() => expect(calls.deliveries.at(-1)?.id).toBe(other));
  expect(frames.filter((frame) => frame.id === other).every((frame) => frame.notice === '' && !frame.deleted)).toBe(true);
  expect(calls.deliveries[0]?.signal.aborted).toBe(true);
});

it('repairs an unmatched delta with a fresh snapshot and resumes after that snapshot’s sequence', async () => {
  render(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  calls.read.mockResolvedValue(askSnapshot({ conversation: askConversation({ state: 'running', seq: 7 }), messages: [askMessage({ content: [{ type: 'text', text: 'A repaired snapshot' }] })] }));
  await deliver({ kind: 'delta', message_id: ASK_ANSWER, offset: 0, text: 'A broken suffix' });
  await waitFor(() => expect(calls.stream).toHaveBeenCalledTimes(2));
  expect(calls.deliveries[1]?.after).toBe(7); expect(calls.deliveries[0]?.signal.aborted).toBe(true);
  expect(screen.getByTestId('answer')).toHaveTextContent('A repaired snapshot'); expect(screen.getByTestId('answer')).not.toHaveTextContent('A broken suffix');
});

it('retires a deleted snapshot and does not immediately fetch it again', async () => {
  render(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  await deliver({ kind: 'deleted' });
  await waitFor(() => expect(screen.getByTestId('deleted')).toHaveTextContent('true'));
  expect(screen.getByTestId('answer')).toHaveTextContent('No snapshot');
  expect(calls.read).toHaveBeenCalledOnce(); expect(calls.deliveries[0]?.signal.aborted).toBe(true);
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps both local context refusal reasons readable in ${locale}`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale);
  render(probe()); await waitFor(() => expect(calls.stream).toHaveBeenCalledOnce());
  const noRoom = { kind: 'error' as const, code: 'ask_context_no_room', message: 'Server fallback' };
  await deliver(noRoom);
  expect(screen.getByTestId('notice')).toHaveTextContent(t('ask.contextNoRoom'));
  const tooLong = { kind: 'error' as const, code: 'ask_context_too_long', model: 'Chosen model', message: 'Server fallback' };
  await deliver(tooLong, ASK_ID, 7);
  expect(screen.getByTestId('notice')).toHaveTextContent(t('ask.contextTooLong', { model: 'Chosen model' }));
  expect(screen.getByTestId('notice')).not.toHaveTextContent('Server fallback');
});
