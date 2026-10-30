/** R1b S25/S27: the real topic tab shows usable agents and defaults; Settings keeps its page, path and section identities. */
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type { StartOptions } from '@/api/start';
import { Breadcrumbs } from '@/features/navigation/Breadcrumbs';
import { TopicAgents } from '@/features/topic/Agents';
import { Topic, type TopicTab } from '@/features/topic/Topic';
import type { Topic as TopicNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { clearScreenHeading, currentHeading } from '@/shell/screen';
import { me, memberAccess, org, userRef } from '../doubles/fixtures';
import { agent, arrangement, model, options, provider } from '../unit/start-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  read: vi.fn<(id: string, environment: string | null) => Promise<StartOptions>>(),
  topic: vi.fn<() => Promise<TopicNode>>(), shares: vi.fn(),
}));
const topic: TopicNode = {
  ...org('login', 'acme.login', 'login', userRef(me(1, 'ada', 'Ada')), {
    access: { ...memberAccess, owns: true, permissions: ['topic.view', 'topic.interact', 'topic.manage'] },
  }),
  kind: 'topic', org_id: 'acme', parent_id: 'general-project', status: 'open', done_at: null, thread_display_state: null,
  environments: [], default_environment_id: null, thread_count: 0,
  attention: { running: 0, waiting_approval: 0, blocked: 0 },
  work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, last_activity_at: null, seq: 1,
  path: [
    { id: 'acme', kind: 'org', name: 'Acme', redacted: false },
    { id: 'general-project', kind: 'project', name: 'General', redacted: false },
    { id: 'login', kind: 'topic', name: 'login', redacted: false },
  ],
};
vi.mock('@/api/start', async (original) => ({ ...await original<object>(), startOptionsQuery: (id: string, environment: string | null) => ({
  queryKey: ['topic-agent-options', id, environment], staleTime: Infinity, queryFn: () => calls.read(id, environment),
}) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), topicQuery: () => ({
  queryKey: ['topic-agent-page'], staleTime: Infinity, queryFn: calls.topic,
}) }));
vi.mock('@/api/share', async (original) => ({ ...await original<object>(), agentSharesQuery: () => ({
  queryKey: ['unwanted-topic-document-policy'], queryFn: () => { calls.shares(); return Promise.resolve([]); },
}) }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<object>(), useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/navigation/favorites', () => ({ StarButton: () => null }));
vi.mock('@/features/announcements/Composer', () => ({ PlaceAnnouncements: () => null }));
vi.mock('@/features/thread/SubscriptionBell', () => ({ SubscriptionBell: () => null }));
vi.mock('@/features/topic/TopicMenu', () => ({ TopicMenu: () => null, TopicHeaderMenu: () => null }));
vi.mock('@/features/topic/TagPicker', () => ({ TagPicker: () => null }));
vi.mock('@/features/topic/Activity', () => ({ TopicActivity: () => null }));
vi.mock('@/features/topic/statusUndo', () => ({ useStatusChange: () => () => Promise.resolve(topic) }));
// The real SettingsFrame and section heading remain; general edit/save has its own native tests.
vi.mock('@/features/settings/org/General', () => ({ General: () => <div data-testid="topic-general-fields" /> }));

const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); calls.read.mockResolvedValue(options()); calls.topic.mockResolvedValue(topic); });
afterEach(async () => {
  clearScreenHeading(); useAppearance.setState(appearance);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  document.documentElement.classList.toggle('dark', wasDark); await setLocale('en');
});
async function look(locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
}
function showAt(ui: ReactNode) {
  const root = createRootRoute({ component: () => ui });
  const route = createRoute({ getParentRoute: () => root, path: '/topic/$topicId', component: () => null });
  const router = createRouter({ routeTree: root.addChildren([route]), history: createMemoryHistory({ initialEntries: ['/topic/login'] }),
    defaultPendingMs: 0, defaultPendingMinMs: 0 });
  return show(<RouterProvider router={router} />);
}
function TopicPage() {
  const [tab, setTab] = useState<TopicTab>('activity');
  return <Topic topicId={topic.id} tab={tab} onTab={setTab} section="general" onSection={() => undefined} />;
}

for (const [locale, theme] of modes) {
  it(`mounts one authorized answer only when Agents is chosen, without document policy (${locale}/${theme})`, async () => {
    await look(locale, theme);
    showAt(<TopicPage />);
    const tabs = await screen.findByRole('tablist', { name: i18n.t('places.topic.sections') });
    expect(calls.read).not.toHaveBeenCalled();
    await userEvent.click(within(tabs).getByRole('tab', { name: i18n.t('people.agentChoices') }));
    expect(await screen.findByRole('link', { name: 'Agent a1' })).toHaveAttribute('href', '/resources/agents/a1');
    expect(screen.getAllByRole('link', { name: 'Agent a1' })).toHaveLength(1);
    expect(screen.getAllByRole('link', { name: 'Agent a2' })).toHaveLength(1);
    expect(screen.getByText(i18n.t('topicAgents.defaultAgent'))).toBeVisible();
    expect(screen.getByText(i18n.t('topicAgents.defaultProvider'))).toBeVisible();
    expect(screen.getByText('M1')).toBeVisible();
    expect(screen.getByText(i18n.t('choiceCatalog.effort.low.name'))).toBeVisible();
    expect(screen.getByRole('link', { name: i18n.t('topicAgents.defaults') })).toHaveAttribute('href', '/topic/login?tab=settings&section=policies');
    expect(screen.queryByText(i18n.t('people.documentsPolicy'))).not.toBeInTheDocument();
    expect(calls.shares).not.toHaveBeenCalled();
    expect(calls.read).toHaveBeenCalledExactlyOnceWith(topic.id, null);
  });

  it(`lists only usable profiles and treats an unavailable default as incomplete (${locale}/${theme})`, async () => {
    await look(locale, theme);
    calls.read.mockResolvedValue(options({ providers: [
      provider('p1', { agents: [agent('blocked', { reason: 'no_use' }), agent('unreported', { reason: 'harness_unreported' }),
        agent('closed', { arrangements: [arrangement('together', 1, false, 'arrangement_not_allowed')] }),
        { ...agent('disabled'), agent: { ...agent('disabled').agent, disabled: true } }, agent('a2')],
        models: [model('m1', ['low', 'high'])] }),
      provider('exhausted', { status: 'exhausted', agents: [agent('quota')] }),
      provider('denied', { usable: false, reason: 'no_use', agents: [agent('private')] }),
    ] }));
    const view = show(<TopicAgents topic={topic} />);
    expect(await screen.findByRole('link', { name: 'Agent a2' })).toBeVisible();
    for (const name of ['blocked', 'unreported', 'closed', 'disabled', 'quota', 'private']) expect(screen.queryByRole('link', { name: `Agent ${name}` })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('topicAgents.defaultAgent'))).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('topicAgents.incomplete'))).toBeVisible();
    expect(view.container).not.toHaveTextContent('harness_unreported');
    expect(view.container).not.toHaveTextContent('arrangement_not_allowed');
    expect(calls.read).toHaveBeenCalledTimes(1);
  });

  it(`keeps failed, empty, unauthorized and archived answers distinct (${locale}/${theme})`, async () => {
    await look(locale, theme);
    calls.read.mockRejectedValueOnce(new Error('unavailable agent inventory')).mockResolvedValue(options({ providers: [] }));
    const view = show(<TopicAgents topic={topic} />);
    const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
    expect(screen.queryByText(i18n.t('topicAgents.empty'))).not.toBeInTheDocument();
    await userEvent.click(retry);
    expect(await screen.findByText(i18n.t('topicAgents.empty'))).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(2);
    view.unmount(); calls.read.mockClear();
    const denied = show(<TopicAgents topic={{ ...topic, access: { ...topic.access, owns: false, permissions: ['topic.view'] } }} />);
    expect(screen.getByText(i18n.t('topicAgents.notAllowed', { owner: topic.owner.display_name }))).toBeVisible();
    expect(calls.read).not.toHaveBeenCalled(); denied.unmount();
    show(<TopicAgents topic={{ ...topic, archived: true }} />);
    expect(screen.getByText(i18n.t('topicAgents.archived'))).toBeVisible();
    expect(calls.read).not.toHaveBeenCalled();
  });

  it(`names the Settings page separately from its General section and ends its path at login (${locale}/${theme})`, async () => {
    await look(locale, theme);
    showAt(<><Breadcrumbs /><Topic topicId={topic.id} tab="settings" onTab={() => undefined} section="general" onSection={() => undefined} /></>);
    expect(await screen.findByRole('heading', { level: 1, name: i18n.t('settings.org.sectionNames.general') })).toBeVisible();
    await waitFor(() => expect(currentHeading()?.tabTitle).toBe(i18n.t('screens.sections.settings')));
    expect(currentHeading()?.title).toBe(i18n.t('screens.sections.settings'));
    expect(currentHeading()?.trail).toBeUndefined();
    const path = screen.getByRole('navigation', { name: i18n.t('ui.breadcrumbs.label') });
    const links = within(path).getAllByRole('link');
    expect(links.at(-1)).toHaveTextContent('login');
    expect(links.at(-1)).toHaveAttribute('href', '/topic/login?tab=overview');
    expect(within(path).queryByText(i18n.t('screens.sections.settings'))).not.toBeInTheDocument();
    expect(screen.getByTestId('topic-general-fields')).toBeVisible();
    expect(calls.read).not.toHaveBeenCalled();
  });
}

it('keeps a pending answer distinct from an empty inventory and shows only the confirmed reply', async () => {
  let finish: ((answer: StartOptions) => void) | undefined;
  calls.read.mockReturnValueOnce(new Promise<StartOptions>((resolve) => { finish = resolve; }));
  const view = show(<TopicAgents topic={topic} />);
  expect(screen.queryByText(i18n.t('topicAgents.empty'))).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-topic-agents]')).toBeNull();
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  if (finish === undefined) throw new Error('the visible panel requested its one answer');
  const reply = finish;
  await act(async () => { reply(options()); await Promise.resolve(); });
  expect(await screen.findByRole('link', { name: 'Agent a1' })).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(1);
});
