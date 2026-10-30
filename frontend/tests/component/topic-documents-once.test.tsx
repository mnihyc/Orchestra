/** UA-30: topic documents live in the page, including when an older layout retained the side view. */
import { DndContext } from '@dnd-kit/core';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as NodesApi from '@/api/nodes';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { Topic, type TopicTab } from '@/features/topic/Topic';
import '@/features/documents/register';
import { ARRANGEMENTS, shippedLayout } from '@/features/workbench/Arrangements';
import { useVisibleLayout } from '@/features/workbench/availability';
import { allTabs, findView, open, type Tab } from '@/features/workbench/layout';
import { SideBarView } from '@/features/workbench/Sections';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { useWorkbench } from '@/features/workbench/store';
import { offers, viewOf } from '@/features/workbench/views';
import type { Topic as TopicNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<TopicNode>>(), side: vi.fn<(tab: Tab) => void>() }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const beforeAppearance = useAppearance.getState();
const beforeWorkbench = useWorkbench.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');

const topic: TopicNode = {
  ...org('release', 'release', 'Release', ADA), kind: 'topic', org_id: 'acme', parent_id: 'project',
  status: 'open', done_at: null, thread_display_state: null, environments: [], default_environment_id: null, thread_count: 0,
  attention: { running: 0, waiting_approval: 0, blocked: 0 },
  work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, last_activity_at: null, seq: 1,
};

vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  topicQuery: () => ({ queryKey: ['single-topic-documents'], queryFn: calls.read, staleTime: Infinity }),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/navigation/NodeMenu', () => ({ useIsPending: () => false }));
vi.mock('@/features/navigation/favorites', () => ({ StarButton: () => null }));
vi.mock('@/features/topic/statusUndo', () => ({ useStatusChange: () => () => Promise.resolve(topic) }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => null }));
vi.mock('@/features/topic/TopicMenu', () => ({ TopicMenu: () => null, TopicHeaderMenu: () => null }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncements: () => null }));
vi.mock('@/features/topic/Activity', () => ({ TopicActivity: () => null }));
// The list has its own route/operation proofs; this case tests the real page tab and workbench eligibility.
vi.mock('@/features/topic/Documents', () => ({ TopicDocuments: () => <article data-testid="topic-documents">Field notes</article> }));
vi.mock('@/features/workbench/choiceHelp', () => ({ useWorkbenchChoiceHelp: () => () => undefined }));

registerShippedViews(() => null);

/** The real side bar reads its addressed navigation area, including when every side section is unavailable. */
function showAt(ui: ReactNode, pathname: string) {
  const root = createRootRoute({ component: () => ui });
  const topicRoute = createRoute({ getParentRoute: () => root, path: '/topic/$topicId', component: () => null });
  const projectRoute = createRoute({ getParentRoute: () => root, path: '/project/$projectId', component: () => null });
  const router = createRouter({ routeTree: root.addChildren([topicRoute, projectRoute]),
    history: createMemoryHistory({ initialEntries: [pathname] }), defaultPendingMs: 0, defaultPendingMinMs: 0 });
  return show(<RouterProvider router={router} />);
}

function TopicPage() {
  const [tab, setTab] = useState<TopicTab>('activity');
  return <Topic topicId={topic.id} tab={tab} onTab={setTab} section="general" onSection={() => undefined} />;
}

function RightBar() {
  const layout = useVisibleLayout();
  return <SideBarView bar="right" layout={layout.right} addView={null} renderView={(tab) => {
    calls.side(tab);
    return <p>Saved side content</p>;
  }} />;
}

beforeEach(() => {
  vi.clearAllMocks();
  calls.read.mockResolvedValue(topic);
});
afterEach(async () => {
  cleanup();
  useWorkbench.setState(beforeWorkbench);
  useAppearance.setState(beforeAppearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

it.each(modes)('offers one page Documents tab and preserves an independently retained side view on the topic in %s/%s', async (locale, theme) => {
  await setLocale(locale);
  useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme;
  document.documentElement.classList.toggle('dark', theme === 'dark');
  // Keep an explicitly retained inspector when another page gains focus.
  const saved = open(shippedLayout('topic'), 'documents', 'right');
  expect(findView(saved, 'documents')).not.toBeNull();
  useWorkbench.setState({ screen: 'topic', layout: saved });
  const view = showAt(<DndContext><TopicPage /><RightBar /></DndContext>, `/topic/${topic.id}`);
  const tabs = await screen.findByRole('tablist', { name: i18n.t('places.topic.sections') });
  const name = locale === 'en' ? 'Documents' : '文档';
  const documents = within(tabs).getByRole('tab', { name });
  expect(screen.getAllByRole('tab', { name })).toHaveLength(1);
  expect(view.container.querySelector('[data-section-view="documents"]')).not.toBeNull();
  expect(screen.getByText('Saved side content')).toBeVisible();
  await userEvent.click(documents);
  expect(documents).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByTestId('topic-documents')).toBeVisible();
  expect(screen.getAllByText('Field notes')).toHaveLength(1);
  expect(findView(useWorkbench.getState().layout, 'documents')).not.toBeNull();
  expect(allTabs(shippedLayout('topic')).some((tab) => tab.view === 'documents')).toBe(false);
  expect(ARRANGEMENTS.topic.flatMap((arrangement) => arrangement.right)).not.toContain('documents');
  expect(calls.read).toHaveBeenCalledTimes(1);

  // The same saved side view remains usable on the other screens; this is not a global removal.
  const definition = viewOf('documents');
  if (definition === undefined) throw new Error('the real Documents feature is registered');
  expect(offers(definition, 'topic', null)).toBe(false);
  for (const kind of ['org', 'project', 'thread'] as const) expect(offers(definition, kind, null)).toBe(true);
  view.unmount();
  useWorkbench.setState({ screen: 'project', layout: saved });
  showAt(<DndContext><RightBar /></DndContext>, '/project/project');
  expect(await screen.findByText('Saved side content')).toBeVisible();
  expect(calls.side.mock.calls.map(([tab]) => tab.view)).toContain('documents');
});
