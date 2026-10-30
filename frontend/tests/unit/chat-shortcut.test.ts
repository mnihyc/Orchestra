/** Retargeting the one chat reveals its chosen position and keeps every surrounding layout choice. */
import { afterEach, expect, it, vi } from 'vitest';
import { shippedLayout } from '@/features/workbench/Arrangements';
import { allTabs, findTab, findView, move, serialize, setTabTarget } from '@/features/workbench/layout';
import { revealView } from '@/features/workbench/openView';
import { useWorkbench } from '@/features/workbench/store';
import { offers, viewOf } from '@/features/workbench/views';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import '@/features/chat/register';

afterEach(() => { useWorkbench.getState().leave(); vi.unstubAllGlobals(); });

it('reuses a collapsed chat for another node, with no navigation, duplicate tab or lost chosen size', () => {
  registerShippedViews(() => null);
  const initial = shippedLayout('thread');
  const first = findView(initial, 'chat');
  if (first === null) throw new Error('No initial chat');
  const placed = move(initial, first.tab.id, { to: 'area', area: 'bottom' });
  const before = serialize(placed);
  const after = setTabTarget(placed, first.tab.id, { node: 'team' });
  expect(serialize(placed)).toBe(before);
  expect(findTab(after, first.tab.id)?.area).toBe('bottom');
  expect(findTab(after, first.tab.id)?.tab.of).toEqual({ node: 'team' });
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 0));
  useWorkbench.setState({ screen: 'thread', layout: placed });
  const address = window.location.href;
  revealView('chat', { node: 'team' }, { focus: false });
  revealView('chat', { node: 'another-team' }, { focus: false });
  const result = useWorkbench.getState().layout;
  expect(allTabs(result).filter((tab) => tab.view === 'chat')).toHaveLength(1);
  expect(findView(result, 'chat', { node: 'another-team' })?.tab.id).toBe(first.tab.id);
  expect(findTab(result, first.tab.id)?.area).toBe('bottom');
  expect(result.bottom.open).toBe(true);
  expect(window.location.href).toBe(address);
});

it('keeps an explicit team target available on a different page, with contextual chat still absent before it is opened', () => {
  const definition = viewOf('chat');
  if (definition === undefined) throw new Error('Chat is not registered');
  useWorkbench.setState({ screen: 'page', layout: shippedLayout('page') });
  expect(offers(definition, 'page', null)).toBe(false);
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 0));
  revealView('chat', { node: 'team' }, { focus: false });
  expect(offers(definition, 'page', { title: 'A different page', views: { ready: true, chat: false } })).toBe(true);
});
