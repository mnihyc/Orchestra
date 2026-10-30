/** Status values retain their source, absent prices and idle states instead of fabricating zeros. */
import { expect, it } from 'vitest';
import { STATUS_ITEMS, outputSpeed } from '@/features/status-items/registry';
import { defaultStatus, moveStatus, readStatus, showStatus, stepStatus } from '@/features/status-items/arrangement';
import { loadWorkspace, saveWorkspace } from '@/features/workbench/persistence';
import { shippedLayout } from '@/features/workbench/Arrangements';
import { useWorkbench } from '@/features/workbench/store';
import { statusContext } from './status-items-fixtures';

it('prefers reported output counts and explicitly marks the streamed estimate', () => {
  const context = statusContext();
  expect(outputSpeed(context)).toEqual({ speed: 48, estimated: false });
  const estimated = { ...context, turn: context.turn === null ? null : { ...context.turn, usage: null }, outputCounts: { ascii: 482, other: 0 } };
  expect(outputSpeed(estimated)).toEqual({ speed: 10, estimated: true });
  expect(STATUS_ITEMS.find((item) => item.id === 'speed')?.read(estimated)?.text).toBe('≈10 tok/s');
  expect(STATUS_ITEMS.find((item) => item.id === 'tokens')?.read(estimated)?.text).toBe('≈100 out');
  expect(STATUS_ITEMS.find((item) => item.id === 'tokens')?.read(estimated)?.details).toEqual([{ name: 'Input', value: '—' }, { name: 'Output', value: '≈100' }]);
  expect(STATUS_ITEMS.find((item) => item.id === 'tokens')?.read(context)?.details).toEqual([{ name: 'Input', value: '100' }, { name: 'Output', value: '480' }]);
  const missing = { ...estimated, tokenEstimate: null };
  expect(outputSpeed(missing)).toBeNull();
  expect(STATUS_ITEMS.find((item) => item.id === 'tokens')?.read(missing)).toBeNull();
  const different = { ...estimated, tokenEstimate: { ascii_prose_x100: 241, other_x100: 132 } };
  expect(outputSpeed(different)).toEqual({ speed: 20, estimated: true });
});

it('omits idle turn measurements and unknown cost, and preserves a priced estimate', () => {
  const context = statusContext(), idle = { ...context, thread: { ...context.thread, state: 'idle' as const } };
  for (const id of ['speed', 'elapsed', 'tokens', 'turn_cost']) expect(STATUS_ITEMS.find((item) => item.id === id)?.read(idle)).toBeNull();
  expect(STATUS_ITEMS.find((item) => item.id === 'thread_cost')?.read(context)).toBeNull();
  context.thread.costs = { total: { credits: '1.25', estimated_credits: '0.25', unrated_tokens: 0 }, turn: null };
  expect(STATUS_ITEMS.find((item) => item.id === 'thread_cost')?.read(context)?.text).toBe('Thread ≈1.25 credits');
  context.thread.costs.total.credits = null;
  expect(STATUS_ITEMS.find((item) => item.id === 'thread_cost')?.read(context)).toBeNull();
});

it('keeps a paused turn’s spent tokens and frozen working time while omitting its speed', () => {
  const context = statusContext();
  if (context.turn === null) throw new Error('The current turn is required');
  context.thread.state = 'waiting_approval';
  context.turn.waiting_since = '2026-10-25T00:00:07Z';
  context.now = Date.parse('2026-10-25T00:10:07Z');
  expect(STATUS_ITEMS.find((item) => item.id === 'speed')?.read(context)).toBeNull();
  expect(STATUS_ITEMS.find((item) => item.id === 'elapsed')?.read(context)?.text).toBe('7s');
  expect(STATUS_ITEMS.find((item) => item.id === 'tokens')?.read(context)?.text).toBe('580 tokens');
});

it('keeps the status arrangement in the same per-person workspace across a reload', () => {
  const layout = shippedLayout('thread');
  const entries = stepStatus(showStatus(moveStatus(defaultStatus(), 'speed', 'bar'), 'tokens', 'live', false), 'provider_window', -1);
  saveWorkspace('status-person', { arrangement: 'default', layout, focusedGroup: null, focusedTab: null, focusedMainGroup: null, focusedMainTab: null, activeContent: null, focusMode: false, closedTabs: [], statusItems: entries });
  expect(loadWorkspace('status-person', 'thread').kept?.statusItems).toEqual(entries);
  expect(loadWorkspace('another-person', 'thread').kept).toBeNull();
  expect(readStatus([{ id: 'speed', place: 'somewhere', shown: true }])).toEqual(defaultStatus());
});

it('resets the saved status arrangement with the existing Layout reset', () => {
  useWorkbench.getState().enter('status-reset-person', 'thread');
  useWorkbench.getState().setStatusItems(moveStatus(defaultStatus(), 'speed', 'header'));
  useWorkbench.getState().reset();
  expect(useWorkbench.getState().statusItems).toEqual(defaultStatus());
  expect(loadWorkspace('status-reset-person', 'thread').kept?.statusItems).toEqual(defaultStatus());
  useWorkbench.getState().leave();
});
