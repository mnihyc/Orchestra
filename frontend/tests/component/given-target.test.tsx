/** Exact input records belong to their own fixed inspector; clearing one filter never changes another target. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ScrollText } from 'lucide-react';
import type * as ModelInputs from '@/api/modelInputs';
import type * as Door from '@/features/files/door';
import type * as Live from '@/realtime/live';
import { GivenView } from '@/features/inspector/Given';
import { openGiven } from '@/features/inspector/givenFocus';
import { allTabs, findTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { registerView } from '@/features/workbench/views';
import { setPanelScope } from '@/features/workbench/panelScope';
import { threadWriter } from '@/realtime/stores/thread';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const targets = vi.hoisted(() => ({ route: 'given-route', preview: 'given-preview', record: 'given-exact-record', queries: vi.fn(), follows: vi.fn() }));
vi.mock('@/features/files/door', async (load) => ({ ...await load<typeof Door>(), useRouteThread: () => targets.route }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: (id: unknown) => { targets.follows(id); } }));
vi.mock('@/features/inspector/InstructionSources', () => ({ InstructionSources: () => null }));
vi.mock('@/api/modelInputs', async (load) => ({ ...await load<typeof ModelInputs>(),
  modelInputsQuery: (thread: string, turn: string | null, record: string | null) => ({
    queryKey: ['given-target-proof', thread, turn, record], initialPageParam: null,
    queryFn: () => { targets.queries(thread, turn, record); return Promise.resolve({ items: [], next_cursor: null }); },
    getNextPageParam: () => undefined,
  }),
}));
let unregister: () => void;
function owned(thread: string) {
  const tab = allTabs(useWorkbench.getState().layout).find((tab) => tab.view === 'given' && tab.scope?.mode === 'fixed' && tab.scope.target.id === thread);
  if (tab === undefined) throw new Error('The explicit Given action must create its targeted inspector');
  return tab;
}
function Pane({ id }: { id: string }) {
  const tab = useWorkbench((state) => findTab(state.layout, id)?.tab);
  return tab === undefined ? null : <section aria-label={id}><GivenView tab={tab} area="right" /></section>;
}
beforeEach(() => {
  threadWriter.clear(); targets.queries.mockClear(); targets.follows.mockClear(); localStorage.clear();
  useWorkbench.getState().leave(); useWorkbench.getState().enter('given-person', 'thread');
  useWorkbench.getState().openTab('page', { href: `/thread/${targets.route}` });
  unregister = registerView({ name: 'given', title: 'threadViews.given.title', about: 'threadViews.given.about', icon: ScrollText, screens: ['thread'], area: 'right', body: GivenView });
  for (const id of [targets.route, targets.preview]) {
    const cut = snapshot(); threadWriter.reset(id, { ...cut, thread: { ...cut.thread, id, name: id } }, false);
  }
});
afterEach(() => { cleanup(); unregister(); useWorkbench.getState().leave(); localStorage.clear(); threadWriter.clear(); });

it('opens the actual preview record and Show all keeps that explicit thread instead of the route', async () => {
  const user = userEvent.setup(); openGiven(targets.preview, { record: targets.record }); const tab = owned(targets.preview);
  show(<Pane id={tab.id} />);
  await waitFor(() => expect(targets.queries).toHaveBeenCalledExactlyOnceWith(targets.preview, null, targets.record));
  expect(targets.follows.mock.calls.every(([id]) => id === targets.preview)).toBe(true);
  expect(await screen.findByText(targets.preview)).toBeVisible();
  await user.click(screen.getByRole('button', { name: /Show all/ }));
  await waitFor(() => expect(targets.queries).toHaveBeenLastCalledWith(targets.preview, null, null));
  expect(owned(targets.preview).id).toBe(tab.id); expect(targets.queries).toHaveBeenCalledTimes(2);
});

it('two fixed inspectors retain independent exact records through selection, clearing and workspace reload', async () => {
  openGiven(targets.route, { record: 'route-record' }); const first = owned(targets.route);
  openGiven(targets.preview, { record: targets.record }); const second = owned(targets.preview);
  const pane = show(<><Pane id={first.id} /><Pane id={second.id} /></>);
  await waitFor(() => expect(targets.queries).toHaveBeenCalledTimes(2));
  expect(targets.queries).toHaveBeenCalledWith(targets.route, null, 'route-record');
  expect(targets.queries).toHaveBeenCalledWith(targets.preview, null, targets.record);
  act(() => openGiven(targets.preview, { turn: 'preview-turn' }, second.id));
  await waitFor(() => expect(targets.queries).toHaveBeenCalledWith(targets.preview, 'preview-turn', null));
  expect(owned(targets.route).of?.givenRecord).toBe('route-record'); expect(owned(targets.preview).id).toBe(second.id);
  await userEvent.click(within(screen.getByRole('region', { name: second.id })).getByRole('button', { name: /Show all/ }));
  expect(owned(targets.route).of?.givenRecord).toBe('route-record');
  pane.unmount(); useWorkbench.getState().flush(); useWorkbench.getState().leave(); useWorkbench.getState().enter('given-person', 'thread');
  expect(owned(targets.route).of?.givenRecord).toBe('route-record'); expect(owned(targets.route).id).toBe(first.id);
  expect(owned(targets.preview).id).toBe(second.id); expect(owned(targets.preview).of?.givenTurn).toBeUndefined();
});

it('Follow active view does not apply a retained record from another source to the route thread', async () => {
  openGiven(targets.preview, { record: targets.record }); const tab = owned(targets.preview); show(<Pane id={tab.id} />);
  await waitFor(() => expect(targets.queries).toHaveBeenCalledExactlyOnceWith(targets.preview, null, targets.record));
  act(() => setPanelScope(tab.id, { mode: 'follow' }));
  await waitFor(() => expect(targets.queries).toHaveBeenCalledWith(targets.route, null, null));
  expect(targets.queries).not.toHaveBeenCalledWith(targets.route, null, targets.record);
});
