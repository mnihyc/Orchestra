/** A streaming thread updates its status from native rows without an HTTP request on any tick. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { parametersQuery } from '@/api/threads';
import { queryClient } from '@/api/query';
import type { ThreadParameters } from '@/generated/api-types';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { defaultStatus, showStatus } from '@/features/status-items/arrangement';
import { useWorkbench } from '@/features/workbench/store';
import { threadWriter } from '@/realtime/stores/thread';
import { statusContext } from '../unit/status-items-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ reads: vi.fn(), now: Date.parse('2026-10-25T00:00:10Z') }));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(), api: { GET: (path: string) => { server.reads(path); return Promise.reject(new Error(`Unexpected read ${path}`)); } } }));
vi.mock('@/lib/time', async (load) => ({ ...await load<object>(), useRunningClock: () => server.now }));
afterEach(() => { useWorkbench.setState({ statusItems: defaultStatus() }); });

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
