/** A streaming thread updates its status from native rows without an HTTP request on any tick. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { parametersQuery } from '@/api/threads';
import { queryClient } from '@/api/query';
import { i18n, setLocale } from '@/i18n';
import type { ThreadParameters } from '@/generated/api-types';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { ContextRing } from '@/features/usage/context/ContextRing';
import { StatusLine } from '@/features/timeline/StatusLine';
import { Plan } from '@/features/timeline/items/Plan';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { defaultStatus, showStatus } from '@/features/status-items/arrangement';
import { useWorkbench } from '@/features/workbench/store';
import { threadWriter } from '@/realtime/stores/thread';
import { statusContext } from '../unit/status-items-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { plan } from '../doubles/thread-rows';

const server = vi.hoisted(() => ({ reads: vi.fn(), now: Date.parse('2026-10-25T00:00:10Z') }));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(), api: { GET: (path: string) => { server.reads(path); return Promise.reject(new Error(`Unexpected read ${path}`)); } } }));
vi.mock('@/lib/time', async (load) => ({ ...await load<object>(), useRunningClock: () => server.now }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<object>(), useSocketStatus: () => ({ phase: 'ready' }) }));
afterEach(async () => { useWorkbench.setState({ statusItems: defaultStatus() }); await setLocale('en'); });

it('draws the running status from its small opening facts without reading settings or model metadata', async () => {
  const context = statusContext();
  if (context.turn === null) throw new Error('The native running turn is required');
  server.reads.mockClear(); server.now = context.now;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['thread-screen-facts', context.thread.id], { silent_after_secs: 600, plan_answered_by: 'writers' });
  threadWriter.reset(context.thread.id, { thread: context.thread, turns: [context.turn] }, false);
  const view = show(<StatusLine threadId={context.thread.id} />, cache);
  try {
    await waitFor(() => expect(view.container.querySelector('[data-status-line]')).toBeVisible());
    expect(server.reads).not.toHaveBeenCalled();
  } finally { view.unmount(); threadWriter.evict(context.thread.id); cache.clear(); }
});

it('keeps the latest proposed plan manager-only from its opening facts without a full settings read', async () => {
  const source = statusContext();
  if (source.turn === null) throw new Error('The native proposed turn is required');
  const thread = { ...source.thread, state: 'idle' as const, current_turn_id: null, pending_inputs: 0,
    access: { ...source.thread.access, permissions: ['thread.view', 'thread.interact'] } };
  const turn = { ...source.turn, state: 'completed' as const, plan: true };
  const proposed = plan('proposed-policy', [], { explanation: 'Proceed with the reviewed plan.' });
  if (proposed.kind !== 'plan') throw new Error('The proposal is a plan item');
  const item = { ...proposed, thread_id: thread.id, turn_id: turn.id };
  const context: TimelineContext = { threadId: thread.id, threadName: thread.name, detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: thread.owner.id, canInteract: true, canManage: false, live: true, lit: null, selection: NOT_PICKING };
  server.reads.mockClear();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['thread-screen-facts', thread.id], { silent_after_secs: 600, plan_answered_by: 'managers' });
  threadWriter.reset(thread.id, { thread, turns: [turn], items: [item] }, false);
  const view = show(<TimelineContextValue.Provider value={context}><Plan item={item} /></TimelineContextValue.Provider>, cache);
  try {
    expect(await screen.findByRole('button', { name: i18n.t('plan.proceed') })).toBeDisabled();
    expect(server.reads).not.toHaveBeenCalled();
  } finally { view.unmount(); threadWriter.evict(thread.id); cache.clear(); }
});

it.each(['en', 'zh-CN'] as const)('shows the activity once in its inspection instead of three stacked copies in %s', async (locale) => {
  await setLocale(locale);
  const context = statusContext();
  if (context.turn === null) throw new Error('The running turn is required');
  const { thread, turn } = context;
  const word = i18n.t('statusItems.generating');
  server.now = context.now;
  threadWriter.reset(thread.id, { thread, turns: [turn] }, false);
  const view = show(<StatusPlace place="live" threadId={thread.id} activeLabel={word} />);
  try {
    await userEvent.click(screen.getByRole('button', { name: word }));
    const card = await screen.findByRole('dialog', { name: word });
    expect(within(card).getAllByText(word, { exact: true })).toHaveLength(1);
  } finally { view.unmount(); threadWriter.evict(thread.id); }
});

it('keeps live usage as a breakdown without a summed request figure or reads on any tick', async () => {
  const context = statusContext();
  if (context.turn === null) throw new Error('The running turn is required');
  const { thread } = context, turn = context.turn;
  server.reads.mockClear(); server.now = context.now;
  const cache = new QueryClient({ defaultOptions: queryClient.getDefaultOptions() });
  const source = { scope: 'default' as const, node: null };
  const parameters: ThreadParameters = { provider: planProvider({ id: thread.binding?.provider.id ?? 'provider' }), approval_mode: 'on_request', approval_reviewer: null,
    model: { value: thread.model, writable: true, options: [], source }, effort: { value: thread.effort, supported: true, writable: true, source }, parameters: [] };
  cache.setQueryData(parametersQuery(thread.id).queryKey, parameters);
  // The line shows what is being done and one clock until a person adds more: here the person has added speed and usage details.
  useWorkbench.setState({ statusItems: showStatus(showStatus(defaultStatus(), 'speed', 'live', true), 'usage', 'live', true) });
  threadWriter.reset(thread.id, { thread, turns: [turn] }, false);
  const view = show(<QueryClientProvider client={cache}><StatusPlace place="bar" threadId={thread.id} /><StatusPlace place="live" threadId={thread.id} /></QueryClientProvider>);
  expect(screen.getByRole('button', { name: 'Output speed' })).toHaveTextContent('48 tok/s');
  expect(server.reads).not.toHaveBeenCalled();
  for (let tick = 1; tick <= 10; tick += 1) {
    act(() => {
      server.now = context.now + tick * 1000;
      threadWriter.apply(thread.id, { thread: { ...thread, seq: thread.seq + tick }, turns: [{ ...turn, seq_updated: thread.seq + tick, usage: { input_tokens: 100, output_tokens: 480 + tick * 48, total_tokens: 580 + tick * 48, cached_input_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 0 } }] });
    });
  }
  expect(screen.getByRole('button', { name: 'Output speed' })).toHaveTextContent('48 tok/s');
  const usage = screen.getByRole('button', { name: 'Turn usage' });
  expect(usage).toHaveTextContent('Usage');
  expect(view.container).not.toHaveTextContent('1,060 tokens');
  await userEvent.click(usage);
  expect(await screen.findByText('100')).toBeVisible();
  expect(screen.getByText('960')).toBeVisible();
  expect(screen.getByText('Cached input')).toBeVisible();
  expect(server.reads).not.toHaveBeenCalled();
  view.unmount(); threadWriter.evict(thread.id); cache.clear();
});

it.each(['en','zh-CN'] as const)('keeps the default context figure in the composer and leaves its extra status placement a deliberate choice in %s',async(locale)=>{
  await setLocale(locale);useWorkbench.setState({statusItems:defaultStatus()});
  const context=statusContext(),thread=context.thread;
  thread.usage={input_tokens:100_000,cached_input_tokens:0,output_tokens:0,reasoning_tokens:0,total_tokens:100_000,context_used:100_000,context_window:200_000,compact_at:null,updated_at:'2026-10-25T00:00:10Z'};
  threadWriter.reset(thread.id,{thread,turns:context.turn===null?[]:[context.turn]},false);
  const view=show(<><ContextRing thread={thread}/><StatusPlace place="bar" threadId={thread.id}/></>);
  try {
    expect(view.container.querySelectorAll('[data-context-ring]')).toHaveLength(1);
    expect(view.container.querySelector('[data-status-item="context"]')).toBeNull();
    act(()=>useWorkbench.setState({statusItems:showStatus(defaultStatus(),'context','bar',true)}));
    expect(view.container.querySelector('[data-status-item="context"]')).not.toBeNull();
  }finally{view.unmount();threadWriter.evict(thread.id);}
});
