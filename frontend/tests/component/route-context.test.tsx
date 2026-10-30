/** Cold saved layouts resolve their addressed resource even when Screen has never been visible. */
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import type * as Live from '@/realtime/live';
import { THREAD_GLYPHS } from '@/ui/ThreadState';
import { RouteContext, RouteTabNames, TargetContext } from '@/shell/RouteContext';
import { queryClient } from '@/api/query';
import { orgBySlugQuery } from '@/api/nodes';
import type * as Nodes from '@/api/nodes';
import type { Org } from '@/generated/api-types';
import { parsePageAddress } from '@/features/workbench/PageView';
import { i18n, setLocale } from '@/i18n';
import { clearScreenHeading, currentHeading, useCurrentHeading } from '@/shell/screen';
import { allTabs, makeGroup, makeTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME, me, org, userRef } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const live = vi.hoisted(() => ({ feed: vi.fn() }));
const orgReads = vi.hoisted(() => ({ pending: false, read: vi.fn<() => Promise<Org>>() }));
vi.mock('@/api/nodes', async (load) => {
  const actual = await load<typeof Nodes>();
  return { ...actual, orgBySlugQuery: (slug: string) => {
    const options = actual.orgBySlugQuery(slug);
    return orgReads.pending ? { ...options, queryFn: orgReads.read } : options;
  } };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: live.feed }));
vi.mock('@/features/navigation/placeLabels', () => ({ useNodeLabel: (_id: string | undefined, title: string) => title }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useParams: () => ({ threadId: ACME.mainThread }), useRouterState: () => ({ pathname: `/thread/${ACME.mainThread}`, search: {} }) }));
function Caption() { return <output aria-label="Frame title">{useCurrentHeading()?.title}</output>; }
afterEach(() => { cleanup(); clearScreenHeading(); threadWriter.evict(ACME.mainThread); useWorkbench.getState().leave(); vi.clearAllMocks(); });

it('resolves the cold thread title, chat context and inactive first-tab icon without mounting Screen or marking the conversation visible', () => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The reader owns this fixture thread');
  const thread = world.threads.thread(person, row);
  const first = makeTab('screen'), chat = makeTab('chat');
  useWorkbench.setState({ layout: { ...useWorkbench.getState().layout, main: { ...makeGroup([first, chat]), active: chat.id } } });
  show(<><RouteContext /><RouteTabNames /><Caption /></>);
  expect(live.feed).toHaveBeenCalledWith(thread.id, false);
  expect(currentHeading()?.target).toBeUndefined();
  act(() => { threadWriter.reset(thread.id, { thread, items: [], turns: [], segments: [] }, false); });
  expect(screen.getByLabelText('Frame title')).toHaveTextContent(thread.name);
  expect(currentHeading()?.nodeId).toBe(thread.id);
  expect(currentHeading()?.path).toEqual(thread.path);
  expect(useWorkbench.getState().named.get(first.id)).toEqual({ title: thread.name, icon: THREAD_GLYPHS[thread.display.state], source: 'view', thread: { id: thread.id, display: thread.display } });
  expect(allTabs(useWorkbench.getState().layout)).toHaveLength(2);
  act(() => { threadWriter.closed(thread.id, 'revoked'); });
  expect(currentHeading()?.target).toBeUndefined();
  expect(currentHeading()?.nodeId).toBeUndefined();
  expect(currentHeading()?.views?.ready).toBe(false);
  expect(screen.getByLabelText('Frame title')).not.toHaveTextContent(thread.name);
});

it.each(['en', 'zh-CN'] as const)('the personal-space frame and tab retain their purpose without another reader in %s', async (locale) => {
  await setLocale(locale);
  const own = org('personal-metadata', 'ada', 'Ada Lovelace', userRef(me(1, 'ada', 'Ada Lovelace')), { org_kind: 'personal' });
  queryClient.setQueryData(orgBySlugQuery('ada').queryKey, own);
  const target = parsePageAddress('/org/ada');
  if (target === null) throw new Error('The personal organization has a native page address.');
  const first = makeTab('screen');
  useWorkbench.setState({ layout: { ...useWorkbench.getState().layout, main: makeGroup([first]) } });
  const requests = vi.spyOn(globalThis, 'fetch');
  const drawn = render(<QueryClientProvider client={queryClient}><TargetContext target={target} /><RouteTabNames /><Caption /></QueryClientProvider>);
  try {
    const title = i18n.t('places.org.personalTitle', { name: own.name });
    expect(await screen.findByLabelText('Frame title')).toHaveTextContent(title);
    expect(useWorkbench.getState().named.get(first.id)?.title).toBe(title);
    expect(currentHeading()?.target).toEqual({ kind: 'org', id: own.id });
    expect(currentHeading()?.views).toEqual({ ready: true, chat: false });
    expect(requests).not.toHaveBeenCalled();
    expect(live.feed).not.toHaveBeenCalled();
  } finally {
    drawn.unmount(); requests.mockRestore(); await queryClient.cancelQueries(); queryClient.clear(); await setLocale('en');
  }
});

it.each([['en', 'hidden'], ['zh-CN', 'hidden'], ['en', 'failed'], ['zh-CN', 'failed']] as const)('a pending frame keeps its already-read personal tab in %s when %s', async (locale, outcome) => {
  await setLocale(locale);
  const own = org('personal-metadata', 'ada', 'Ada Lovelace', userRef(me(1, 'ada', 'Ada Lovelace')), { org_kind: 'personal' });
  const target = parsePageAddress('/org/ada');
  if (target === null) throw new Error('The personal organization has a native page address.');
  const first = makeTab('screen'), title = i18n.t('places.org.personalTitle', { name: own.name });
  useWorkbench.setState({ layout: { ...useWorkbench.getState().layout, main: makeGroup([first]) } });
  useWorkbench.getState().setNamed(first.id, { title, source: 'cache' });
  let finish!: (value: Org) => void;
  let fail!: (error: Error) => void;
  orgReads.pending = true;
  orgReads.read.mockImplementation(() => new Promise((resolve, reject) => { finish = resolve; fail = reject; }));
  const drawn = render(<QueryClientProvider client={queryClient}><TargetContext target={target} /><RouteTabNames /><Caption /></QueryClientProvider>);
  try {
    await waitFor(() => expect(orgReads.read).toHaveBeenCalledTimes(1));
    expect(useWorkbench.getState().named.get(first.id)?.title).toBe(title);
    if (outcome === 'hidden') {
      drawn.unmount();
      expect(useWorkbench.getState().named.get(first.id)?.title).toBe(title);
      await act(async () => { finish(own); await Promise.resolve(); });
      expect(useWorkbench.getState().named.get(first.id)?.title).toBe(title);
    } else {
      await act(async () => { fail(new Error('The current organization could not be read.')); await Promise.resolve(); });
      await waitFor(() => expect(useWorkbench.getState().named.get(first.id)?.title).toBe(i18n.t('screens.org.title')));
      expect(currentHeading()?.views).toEqual({ ready: false, chat: false });
      expect(currentHeading()?.target).toBeUndefined();
    }
    expect(live.feed).not.toHaveBeenCalled();
  } finally {
    drawn.unmount(); orgReads.pending = false; orgReads.read.mockReset(); await queryClient.cancelQueries(); queryClient.clear(); await setLocale('en');
  }
});
