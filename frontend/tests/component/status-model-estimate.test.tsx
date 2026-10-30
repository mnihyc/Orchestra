/** Live status uses the running turn's model estimate without making a model-name guess. */
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { parametersQuery } from '@/api/threads';
import { useStatusValues } from '@/features/status-items/useValues';
import type { Thread, ThreadParameters, Turn } from '@/generated/api-types';
import { newOutputCounts, type OutputCounts } from '@/realtime/outputCounts';
import { ACME } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { World } from '../doubles/world';

let current: { thread: Thread; turn: Turn; items: Map<string, unknown>; outputCounts: OutputCounts };
vi.mock('@/realtime/stores/thread', () => ({
  runningTurn: (view: typeof current) => view.turn,
  useThreadPart: (_id: string, select: (view: typeof current) => unknown) => select(current),
}));
vi.mock('@/features/workbench/store', () => ({ useWorkbench: (select: (value: { statusItems: { id: string; place: string; shown: boolean }[] }) => unknown) =>
  select({ statusItems: [{ id: 'speed', place: 'live', shown: true }] }) }));
vi.mock('@/features/resource-facts/live', () => ({ useResourceSample: () => null }));
vi.mock('@/lib/time', async (load) => ({ ...await load<object>(), useRunningClock: () => Date.parse('2026-10-25T00:00:10Z') }));

function Values({ id }: { id: string }) {
  return <>{useStatusValues(id, 'live').map((item) => <span key={item.id}>{item.value.text}</span>)}</>;
}

function parameters(options: ThreadParameters['model']['options']): ThreadParameters {
  return {
    approval_mode: 'on_request', approval_modes: ['on_request'], approval_reviewer: null,
    effort: { value: null, writable: false, supported: false, source: { scope: 'default', node: null } },
    model: { options, value: null, writable: true, source: { scope: 'default', node: null } }, parameters: [],
  };
}

afterEach(() => cleanup());

it('selects the running model, updates from its report and omits an unavailable estimate', async () => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The running thread fixture is required');
  const active = turn('estimate-turn', { ordinal: 1, state: 'running', inputId: 'input', startedAt: '2026-10-25T00:00:00Z' });
  active.model = 'running-model'; active.usage = null;
  const thread = { ...world.threads.thread(person, row), state: 'running' as const, current_turn_id: active.id, model: 'next-model' };
  const items = new Map<string, unknown>();
  // The held world may be large; a status placement must read neither its rows nor its bodies.
  for (let index = 0; index < 1_000_000; index += 1) items.set(String(index), null);
  const values = vi.spyOn(items, 'values').mockImplementation(() => { throw new Error('History must not be traversed'); });
  const iterate = vi.spyOn(items, Symbol.iterator).mockImplementation(() => { throw new Error('History must not be copied'); });
  current = { thread, turn: active, items, outputCounts: { ...newOutputCounts(active.id), counts: { ascii: 482, other: 0 } } };
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const estimate = { ascii_prose_x100: 482, ascii_structured_x100: 350, other_x100: 100, image_tokens: 1000, measured: true };
  const first = { model_name: 'running-model', display_name: 'Running', allowed: true, reason: null, parameters: {}, token_estimate: estimate };
  const second = { ...first, model_name: 'next-model', token_estimate: { ...estimate, ascii_prose_x100: 241 } };
  const options = [first, second];
  client.setQueryData(parametersQuery(thread.id).queryKey, parameters(options));
  render(<QueryClientProvider client={client}><Values id={thread.id} /></QueryClientProvider>);
  expect(screen.getByText('≈10 tok/s')).toBeVisible();
  await act(async () => {
    client.setQueryData(parametersQuery(thread.id).queryKey, parameters([{ ...first, token_estimate: { ...estimate, ascii_prose_x100: 241 } }, second]));
    await Promise.resolve();
  });
  expect(await screen.findByText('≈20 tok/s')).toBeVisible();
  await act(async () => {
    client.setQueryData(parametersQuery(thread.id).queryKey, parameters([second]));
    await Promise.resolve();
  });
  await waitFor(() => expect(screen.queryByText('≈20 tok/s')).not.toBeInTheDocument());
  expect(items.size).toBe(1_000_000);
  expect(values).not.toHaveBeenCalled();
  expect(iterate).not.toHaveBeenCalled();
  client.clear();
});
