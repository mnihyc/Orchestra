/** The resource landing names all kinds, exposes Add and shares one count read with lazy navigation. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createElement, type ReactNode } from 'react';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ResourceLanding } from '@/features/resources/Landing';
import { ResourcesView } from '@/features/navigation/Resources';
import { client } from '@/api/client';
import { ViewOpeningContext, ViewRouteContext } from '@/features/workbench/viewOpening';
import { show } from './support';

const world = vi.hoisted(() => ({ summary: vi.fn(), names: vi.fn<(kind: string) => void>(), navigate: vi.fn(), path: '/resources' }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, ...props }: { children: ReactNode; to: string }) => createElement('a', { ...props, href: to }, children),
  useNavigate: () => world.navigate,
  useRouterState: () => world.path,
}));
vi.mock('@/api/resource-summary', () => ({ resourceSummaryQuery: (opening: string) => ({ queryKey: ['resource-summary', opening], staleTime: Infinity, queryFn: () => {
  world.summary(); return Promise.resolve({ providers: 2, agents: 51, machines: 0, environments: 3, templates: 1, hosts: 1, mcp: 0, mounts: 4, skills: 5 });
} }) }));
vi.mock('@/api/everything', () => ({ everythingQuery: (_reader: string, _parent: string, filter: { kinds: string[] }) => ({
  queryKey: ['resource-navigation', filter.kinds], initialPageParam: null, getNextPageParam: () => null,
  queryFn: () => { const kind = filter.kinds[0] ?? ''; world.names(kind); return Promise.resolve({ items: [{ id: 'reviewer', name: 'Reviewer' }], next_cursor: null }); },
}) }));
vi.mock('@/features/resources/agents/AddAgent', () => ({ AddAgentDialog: ({ onClose }: { onClose: () => void }) => <div role="dialog" aria-label="Agent creation"><button onClick={onClose}>Close</button></div> }));

const appearance = useAppearance.getState();
afterEach(async () => { cleanup(); vi.clearAllMocks(); localStorage.clear(); world.path = '/resources'; client.session.setState({ userId: null }); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`shows all nine links with their exact counts and Add, without reading any list in ${locale}/${theme}`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  client.session.setState({ userId: 'ada' });
  show(<><ResourceLanding /><ResourcesView /></>);
  expect(screen.getByRole('heading', { level: 1, name: i18n.t('screens.sections.resources') })).toBeVisible();
  const landing = within(screen.getByRole('list', { name: locale === 'en' ? 'Kinds of resources' : i18n.t('resources.nav.label') }));
  await waitFor(() => expect(landing.getAllByRole('listitem')[1]).toHaveTextContent('51'));
  expect(landing.getAllByRole('listitem')).toHaveLength(9);
  expect(landing.getAllByRole('link').map((link) => link.getAttribute('href'))).toEqual([
    '/resources/providers', '/resources/agents', '/resources/machines', '/resources/environments', '/resources/templates', '/resources/hosts', '/resources/mcp', '/resources/mounts', '/resources/skills',
  ]);
  expect(landing.getAllByRole('button')).toHaveLength(9);
  for (const button of landing.getAllByRole('button')) expect(button).toHaveTextContent(locale === 'en' ? 'Add' : '添加');
  expect(world.summary).toHaveBeenCalledTimes(1);
  expect(world.names).not.toHaveBeenCalled();
  fireEvent.click(landing.getByRole('button', { name: i18n.t('resourceCreation.agents') }));
  expect(await screen.findByRole('dialog', { name: 'Agent creation' })).toBeVisible();
  expect(world.names).not.toHaveBeenCalled();
});

it('loads the expanded kind only, and keeps its full native count when just one name page is loaded', async () => {
  client.session.setState({ userId: 'ada' });
  show(<ResourcesView />);
  const tree = await screen.findByRole('tree');
  const agents = within(tree).getByRole('treeitem', { name: /Agents/ });
  agents.focus(); fireEvent.keyDown(agents, { key: 'ArrowRight' });
  expect(await within(tree).findByRole('link', { name: 'Reviewer' })).toBeVisible();
  expect(agents).toHaveTextContent('51');
  expect(world.names.mock.calls.map(([kind]) => kind)).toEqual(['agent']);
});

it('shares the count with a later-loaded page and takes a fresh aggregate only on the next screen opening', async () => {
  client.session.setState({ userId: 'ada' });
  const view = show(<ViewRouteContext.Provider value="first-opening"><ResourcesView /></ViewRouteContext.Provider>);
  await waitFor(() => expect(screen.getByRole('treeitem', { name: /Agents/ })).toHaveTextContent('51'));
  view.rerender(<ViewRouteContext.Provider value="first-opening"><ResourcesView /><ResourceLanding /></ViewRouteContext.Provider>);
  await screen.findByRole('list', { name: 'Kinds of resources' });
  expect(world.summary).toHaveBeenCalledTimes(1);
  view.rerender(<ViewRouteContext.Provider value="next-opening"><ResourcesView /><ResourceLanding /></ViewRouteContext.Provider>);
  await waitFor(() => expect(world.summary).toHaveBeenCalledTimes(2));
  expect(world.names).not.toHaveBeenCalled();
});

it('never expands visited kinds or resurrects legacy automatic expansions, and reads only deliberate visible branches', async () => {
  client.session.setState({ userId: 'ada' });
  localStorage.setItem('orch.ada.resources.open', JSON.stringify(['hosts', 'agents', 'templates', 'environments']));
  world.path = '/resources/hosts/host-one';
  const draw = (shown: boolean) => <ViewOpeningContext.Provider value={shown ? 'resources' : null}><ResourcesView /></ViewOpeningContext.Provider>;
  const view = show(draw(true));
  await waitFor(() => expect(screen.getByRole('treeitem', { name: /Agents/ })).toHaveTextContent('51'));
  expect(world.names).not.toHaveBeenCalled();
  world.path = '/resources/templates/template-one'; view.rerender(draw(true));
  expect(world.names).not.toHaveBeenCalled();
  const agents = screen.getByRole('treeitem', { name: /Agents/ });
  agents.focus(); fireEvent.keyDown(agents, { key: 'ArrowRight' });
  expect(await screen.findByRole('link', { name: 'Reviewer' })).toBeVisible();
  expect(world.names.mock.calls.map(([kind]) => kind)).toEqual(['agent']);
  world.path = '/resources/mcp/server-one'; view.rerender(draw(true));
  expect(world.names.mock.calls.map(([kind]) => kind)).toEqual(['agent']);
  view.rerender(draw(false));
  await act(async () => { await Promise.resolve(); });
  expect(world.names.mock.calls.map(([kind]) => kind)).toEqual(['agent']);
  expect(JSON.parse(localStorage.getItem('orch.ada.resources.expanded') ?? '[]')).toEqual(['agents']);
});
