/** A delivered question is ready without its legacy lookup, and the POST result settles it in place. */
import { act, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as ApprovalsApi from '@/api/approvals';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { ApiError, client } from '@/api/client';
import { threadWriter } from '@/realtime/stores/thread';
import { useDecision } from '@/features/approvals/Decision';
import { approvalKey } from '@/api/approvals';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import type { Approval, Item } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { me, userRef } from '../doubles/fixtures';
import { ALLOW_DENY, approvalItem, harnessApproval, otherTool } from '../doubles/thread-rows';
import { show } from './support';

const server = vi.hoisted(() => ({ read: vi.fn<() => Promise<Approval>>(), decide: vi.fn<(id: string, decision: unknown) => Promise<Approval>>() }));
vi.mock('@/api/approvals', async (load) => ({ ...await load<typeof ApprovalsApi>(), approvalQuery: (id: string) => ({ queryKey: ['approval', id], queryFn: server.read }), decideApproval: server.decide }));
const context: TimelineContext = { threadId: 'question-thread', threadName: 'Login', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
function request(canDecide = true, title = 'Choose scope'): Approval {
  return harnessApproval('question-1', { threadId: context.threadId, itemId: 'question-item', title, requester: userRef(me(1, 'ada', 'Ada')), canDecide,
    request: { request_kind: 'user_input', detail: { questions: [{ id: 'scope', header: 'Scope', question: 'Which part should I change?', multiple: false, options: null, allow_other: true }] }, options: [] } });
}
function item(approval: Approval): Extract<Item, { kind: 'approval' }> {
  const row = approvalItem('question-item', approval.id);
  if (row.kind !== 'approval') throw new Error('The fixture is a request item');
  return { ...row, payload: { ...row.payload, approval } };
}
let locale = currentLocale();
let theme: string | null = null;
beforeEach(() => { locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); server.read.mockReset(); server.decide.mockReset(); });
afterEach(async () => { await setLocale(locale); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('delivered question %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); document.documentElement.setAttribute('data-theme', look); });
  it('shows a matching subject once while retaining the question field name', () => {
    const subject = 'Which part should I change?';
    const approval = request(true, subject);
    show(<TimelineContextValue.Provider value={context}><ApprovalCard item={item(approval)} /></TimelineContextValue.Provider>);
    expect(screen.getAllByRole('heading', { name: subject })).toHaveLength(1);
    expect(screen.getByText(subject, { selector: 'legend > span' })).toHaveClass('sr-only');
    expect(screen.getByRole('textbox', { name: subject })).toBeEnabled();
    expect(server.read).not.toHaveBeenCalled();
  });
  it.each(['approved', 'answered'] as const)('renders immediately, sends once and keeps the confirmed %s outcome over the old item', async (state) => {
    const approval = request();
    server.decide.mockResolvedValue({ ...approval, state, can_decide: false, decided_at: approval.created_at, decision: { answers: { scope: ['Login'] } } });
    const view = show(<TimelineContextValue.Provider value={context}><ApprovalCard item={item(approval)} /></TimelineContextValue.Provider>);
    const input = screen.getByRole('textbox', { name: 'Which part should I change?' });
    expect(input).toBeEnabled();
    expect(server.read).not.toHaveBeenCalled();
    await userEvent.type(input, 'Login');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('approvals.sendAnswers') }));
    await waitFor(() => expect(server.decide).toHaveBeenCalledExactlyOnceWith(approval.id, { answers: { scope: ['Login'] } }));
    await waitFor(() => expect(screen.queryByRole('textbox')).not.toBeInTheDocument());
    view.rerender(<TimelineContextValue.Provider value={context}><ApprovalCard item={item(approval)} /></TimelineContextValue.Provider>);
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { expanded: false, name: /Choose scope/ }));
    expect(screen.getByText('Login', { exact: true })).toBeVisible();
    expect(server.read).not.toHaveBeenCalled();
  });
  it('shows the question and waiting sentence to a viewer from that reader’s own decision authority', () => {
    show(<TimelineContextValue.Provider value={{ ...context, canInteract: false, canManage: false }}><ApprovalCard item={item(request(false))} /></TimelineContextValue.Provider>);
    expect(screen.getByRole('textbox', { name: 'Which part should I change?' })).toBeDisabled();
    expect(screen.getByRole('button', { name: i18n.t('approvals.sendAnswers') })).toBeDisabled();
    expect(screen.getByText(i18n.t('approvals.waitingForAnswer'))).toBeVisible();
    expect(server.read).not.toHaveBeenCalled();
    expect(server.decide).not.toHaveBeenCalled();
  });

  it.each(['committed item', 'confirmed answer'] as const)('shows a legacy request’s %s immediately instead of calling it Waiting', (from) => {
    const approval = request();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    const confirmed: Approval = { ...approval, state: 'approved', can_decide: false, decision: { answers: { scope: ['Login'] } } };
    cache.setQueryData(approvalKey(approval.id), from === 'confirmed answer' ? confirmed : approval);
    const legacy: Extract<Item, { kind: 'approval' }> = { ...item(approval), status: from === 'committed item' ? 'approved' : 'pending', payload: { approval_id: approval.id } };
    show(<QueryClientProvider client={cache}><TimelineContextValue.Provider value={context}><ApprovalCard item={legacy} /></TimelineContextValue.Provider></QueryClientProvider>);
    expect(screen.getByRole('button', { name: /Choose scope/ })).toHaveTextContent(i18n.t('approvals.answered'));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('approvals.waiting'), { exact: true })).not.toBeInTheDocument();
    expect(server.read).not.toHaveBeenCalled();
  });

  it('takes a later live cancellation over an older confirmed approval in the query cache', () => {
    const approval = request();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(approvalKey(approval.id), { ...approval, state: 'approved', can_decide: false });
    const cancelled = item({ ...approval, state: 'cancelled', can_decide: false });
    show(<QueryClientProvider client={cache}><TimelineContextValue.Provider value={context}><ApprovalCard item={{ ...cancelled, status: 'cancelled' }} /></TimelineContextValue.Provider></QueryClientProvider>);
    expect(screen.getByRole('button', { name: /Choose scope/ })).toHaveTextContent(i18n.t('approvals.cancelled'));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(server.read).not.toHaveBeenCalled();
  });

  it.each([false, true])('keeps the native card usable over a thin foreign cache and resolves its actual answer in place (question: %s)', async (question) => {
    const pending = question ? request() : harnessApproval('question-1', { threadId: context.threadId, itemId: 'question-item', title: 'Run checks', requester: userRef(me(1, 'ada', 'Ada')),
      request: { request_kind: 'command', detail: { command: 'check redirect', cwd: '/workspace', reason: null }, options: ALLOW_DENY } });
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(approvalKey(pending.id), { id: pending.id, kind: 'thread', name: 'An act’s related place' });
    const settled: Approval = { ...pending, state: 'approved', can_decide: false, decided_at: pending.created_at,
      decision: question ? { answers: { scope: ['Login'] } } : { option_id: 'allow_once' } };
    server.decide.mockResolvedValue(settled);
    const frame = (row: Extract<Item, { kind: 'approval' }>) => <QueryClientProvider client={cache}><TimelineContextValue.Provider value={context}><ApprovalCard item={row} /></TimelineContextValue.Provider></QueryClientProvider>;
    const view = show(frame(item(pending)));
    if (question) await userEvent.type(screen.getByRole('textbox', { name: 'Which part should I change?' }), 'Login');
    const label = question ? i18n.t('approvals.sendAnswers') : `${i18n.t('approvals.allowOnce')} · check redirect`;
    await userEvent.click(screen.getByRole('button', { name: label }));
    await waitFor(() => expect(screen.getByRole('button', { name: new RegExp(pending.title), expanded: false })).toHaveTextContent(question ? i18n.t('approvals.answered') : i18n.t('approvals.allowOnce')));
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(server.decide).toHaveBeenCalledExactlyOnceWith(pending.id, settled.decision);
    view.rerender(frame(item(pending)));
    expect(screen.queryByRole('button', { name: label })).not.toBeInTheDocument();
    expect(cache.getQueryData(approvalKey(pending.id))).toEqual(settled);
    expect(server.read).not.toHaveBeenCalled();
  });

  it('ignores an incomplete live snapshot and recovers that card from its own reader, with an explicit retry if that answer is thin too', async () => {
    const approval = request();
    const broken = { ...approval };
    Reflect.deleteProperty(broken, 'request');
    server.read.mockResolvedValueOnce(broken).mockResolvedValueOnce(approval);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    show(<QueryClientProvider client={cache}><TimelineContextValue.Provider value={context}><ApprovalCard item={item(broken)} /></TimelineContextValue.Provider></QueryClientProvider>);
    expect(await screen.findByText(i18n.t('approvals.incomplete'))).toBeVisible();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(server.decide).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByRole('textbox', { name: 'Which part should I change?' })).toBeEnabled();
    expect(server.read).toHaveBeenCalledTimes(2);
    expect(server.decide).not.toHaveBeenCalled();
  });
});


it.each(['success', 'already decided'] as const)('does not publish a previous reader’s late %s response', async (result) => {
  const before = client.session.getState();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onDecided = vi.fn(), onLost = vi.fn();
  const approval = { ...request(), state: 'approved' as const, can_decide: false };
  let finish!: (value: Approval) => void, refuse!: (error: Error) => void;
  server.decide.mockImplementation(() => new Promise((resolve, reject) => { finish = resolve; refuse = reject; }));
  function Control() {
    const decision = useDecision(approval.id, { onDecided, onLost });
    return <button onClick={() => decision.decide({ answers: { scope: ['Login'] } })}>Send</button>;
  }
  const drawn = show(<QueryClientProvider client={cache}><Control /></QueryClientProvider>);
  try {
    act(() => { client.session.setState({ status: 'signed_in', userId: 'answer-author', readerVersion: 100 }); });
    await userEvent.click(screen.getByRole('button', { name: /^Send$/ }));
    const mutation = cache.getMutationCache().getAll()[0];
    if (mutation === undefined) throw new Error('The actual decision mutation must be pending.');
    act(() => { client.session.setState({ userId: 'different-reader', readerVersion: 101 }); cache.clear(); });
    await act(async () => {
      if (result === 'success') finish(approval);
      else refuse(new ApiError({ type: 'orch:error:approval.already_decided', code: 'approval.already_decided', title: 'Decided', detail: 'Already answered', status: 409, request_id: 'late-answer', current: approval }, null));
      await Promise.resolve();
    });
    await waitFor(() => expect(mutation.state.status).toBe(result === 'success' ? 'success' : 'error'));
    expect(cache.getQueryData(approvalKey(approval.id))).toBeUndefined();
    expect(onDecided).not.toHaveBeenCalled(); expect(onLost).not.toHaveBeenCalled();
  } finally { drawn.unmount(); cache.clear(); client.session.setState(before); }
});

it('keeps a late response on its addressed request when a kept decision view changes target', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onDecided = vi.fn();
  const approval = { ...request(), state: 'approved' as const, can_decide: false };
  let finish!: (value: Approval) => void;
  server.decide.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  function Control({ id }: { id: string }) {
    const decision = useDecision(id, { onDecided });
    return <button onClick={() => decision.decide({ answers: { scope: ['Login'] } })}>Send</button>;
  }
  const frame = (id: string) => <QueryClientProvider client={cache}><Control id={id} /></QueryClientProvider>;
  const drawn = show(frame(approval.id));
  try {
    await userEvent.click(screen.getByRole('button', { name: /^Send$/ }));
    const mutation = cache.getMutationCache().getAll()[0];
    if (mutation === undefined) throw new Error('The actual decision mutation must be pending.');
    drawn.rerender(frame('another-question'));
    await act(async () => { finish(approval); await Promise.resolve(); });
    await waitFor(() => expect(mutation.state.status).toBe('success'));
    expect(cache.getQueryData(approvalKey(approval.id))).toEqual(approval);
    expect(cache.getQueryData(approvalKey('another-question'))).toBeUndefined();
    expect(onDecided).not.toHaveBeenCalled();
  } finally { drawn.unmount(); cache.clear(); }
});


it('keeps native input/result beneath the question’s work detail without another request', async () => {
  const approval = request();
  const source = { ...otherTool('native-question', { name: 'AskUserQuestion', args: { question: 'Native input marker' }, result: { answer: 'Native answer marker' }, turn: 'question-turn' }), thread_id: context.threadId };
  const card = { ...item(approval), thread_id: context.threadId, turn_id: source.turn_id, parent_item_id: source.id };
  threadWriter.reset(context.threadId, { items: [source, card] }, false);
  const network = vi.spyOn(globalThis, 'fetch');
  const drawn = show(<TimelineContextValue.Provider value={context}><ApprovalCard item={card} /></TimelineContextValue.Provider>);
  try {
    expect(screen.queryByText('AskUserQuestion')).not.toBeInTheDocument();
    expect(network).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText(i18n.t('thread.header.workDetail'), { exact: true }));
    expect(await screen.findByText('AskUserQuestion')).toBeVisible();
    expect(screen.getByText(/Native input marker/)).toBeVisible();
    expect(screen.getByText(/Native answer marker/)).toBeVisible();
    expect(network).not.toHaveBeenCalled();
    expect(server.read).not.toHaveBeenCalled();
  } finally { drawn.unmount(); network.mockRestore(); threadWriter.evict(context.threadId); }
});
