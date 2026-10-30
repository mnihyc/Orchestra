/** Explicit rail actions dock the selected view, independently of a hidden bar's temporary peek. */
import { DndContext } from '@dnd-kit/core';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type * as Router from '@tanstack/react-router';
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { forwardRef, type AnchorHTMLAttributes, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { orgsQuery } from '@/api/nodes';
import { makeContainer, makeTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { i18n, setLocale } from '@/i18n';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { Rail } from '@/shell/Rail';
import { useSelectedOrg } from '@/shell/org';
import { TooltipProvider } from '@/ui/Tooltip';
import { ACME, me, org, userRef } from '../doubles/fixtures';

const route = vi.hoisted(() => ({ path: '/org/acme', navigate: vi.fn(() => Promise.resolve()) }));
interface LinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'children'> {
  to: string;
  params?: { slug: string };
  activeOptions?: unknown;
  activeProps?: unknown;
  children: ReactNode | ((state: { isActive: boolean }) => ReactNode);
}
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useNavigate: () => route.navigate,
  useRouterState: ({ select }: { select: (state: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: route.path } }),
  Link: forwardRef<HTMLAnchorElement, LinkProps>(function Link({ to, params, children, activeOptions: _options, activeProps: _active, ...props }, ref) {
    const href = params === undefined ? to : to.replace('$slug', params.slug);
    return <a {...props} ref={ref} href={href}>{typeof children === 'function' ? children({ isActive: route.path === href }) : children}</a>;
  }),
}));
vi.mock('@/features/access/AdminMode', () => ({ useManagementAvailable: () => false }));
vi.mock('@/features/inbox/Counters', () => ({ useInboxCounters: () => ({ unread: 0, approvalsPending: 0 }), useCounterWords: () => ({ unread: null, approvals: null }), CountBadge: () => null }));
vi.mock('@/features/navigation/counts', () => ({ useUnreadChats: () => 0 }));
vi.mock('@/features/navigation/currentOrg', () => ({ useNavigationOrg: () => useSelectedOrg((state) => state.orgId) }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: vi.fn() }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabels: (places: readonly { id: string; name: string }[]) => new Map(places.map((place) => [place.id, place.name])) }));
vi.mock('@/features/navigation/Resources', () => ({ RESOURCES_VIEW: 'resource-navigation' }));
vi.mock('@/shell/keys', () => ({ useActionKeys: () => undefined }));
vi.mock('@/shell/screen', () => ({ useCurrentHeading: () => null }));
vi.mock('@/realtime/stores/tree', () => ({ useNodePath: () => ({ orgId: ACME.org }) }));

const before = useWorkbench.getState();
beforeEach(() => {
  route.path = '/org/acme';
  route.navigate.mockClear();
  useSelectedOrg.setState({ orgId: ACME.org });
  useWorkbench.setState({ ...before, layout: { ...before.layout, left: { open: false, size: 280, active: 'navigation', containers: [
    makeContainer('navigation', [makeTab('navigation')]),
    makeContainer('resource-navigation', [makeTab('resource-navigation')]),
  ] } } });
});
afterEach(async () => { cleanup(); useWorkbench.setState(before); useSelectedOrg.setState({ orgId: null }); await setLocale('en'); });

function showRail() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(orgsQuery.queryKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
  return render(<QueryClientProvider client={client}><FrameReadiness.Provider value={{ ready: false, pending: null }}>
    <TooltipProvider><DndContext><Rail onShortcuts={vi.fn()} /></DndContext></TooltipProvider>
  </FrameReadiness.Provider></QueryClientProvider>);
}

it.each(['en', 'zh-CN'] as const)('docks Resources on click and hides its docked front view on the next click in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup(); showRail();
  const resources = screen.getByRole('button', { name: i18n.t('workbench.choices.containers.resource-navigation.name') });
  expect(resources).toHaveAttribute('aria-pressed', 'false');
  act(() => resources.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('workbench.bars.show.resources'));
  await user.click(resources);
  expect(useWorkbench.getState().layout.left).toMatchObject({ open: true, active: 'resource-navigation' });
  expect(useWorkbench.getState().overlay).toBeNull();
  expect(resources).toHaveAttribute('aria-pressed', 'true');
  await user.tab(); act(() => resources.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('workbench.bars.hide.resources'));
  await user.keyboard('{Enter}');
  expect(useWorkbench.getState().layout.left.open).toBe(false);
  expect(useWorkbench.getState().overlay).toBeNull();
  expect(resources).toHaveAttribute('aria-pressed', 'false');
});

it.each(['en', 'zh-CN'] as const)('docks the org tree on click, switches from Resources and hides it only at its home in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup(); showRail();
  const organization = screen.getByRole('link', { name: 'Acme' });
  const resources = screen.getByRole('button', { name: i18n.t('workbench.choices.containers.resource-navigation.name') });
  expect(organization).toHaveAttribute('aria-expanded', 'false');
  act(() => organization.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('workbench.orgActions.showName', { name: 'Acme' }));
  await user.click(organization);
  expect(useWorkbench.getState().layout.left).toMatchObject({ open: true, active: 'navigation' });
  expect(useWorkbench.getState().overlay).toBeNull();
  expect(organization).toHaveAttribute('aria-expanded', 'true');
  await user.click(resources);
  expect(useWorkbench.getState().layout.left).toMatchObject({ open: true, active: 'resource-navigation' });
  await user.click(organization);
  expect(useWorkbench.getState().layout.left).toMatchObject({ open: true, active: 'navigation' });
  expect(resources).toHaveAttribute('aria-pressed', 'false');
  await user.tab(); act(() => organization.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('workbench.orgActions.hideName', { name: 'Acme' }));
  await user.keyboard('{Enter}');
  expect(useWorkbench.getState().layout.left.open).toBe(false);
  expect(organization).toHaveAttribute('aria-expanded', 'false');
  expect(route.navigate).not.toHaveBeenCalled();
});

it.each(['navigation', 'resource-navigation'])('docks %s when its edge peek was already visible', async (view) => {
  useWorkbench.setState({ overlay: 'left', layout: { ...useWorkbench.getState().layout, left: { ...useWorkbench.getState().layout.left, active: view } } });
  const user = userEvent.setup(); showRail();
  const control = view === 'navigation' ? screen.getByRole('link', { name: 'Acme' }) : screen.getByRole('button', { name: i18n.t('workbench.choices.containers.resource-navigation.name') });
  await user.click(control);
  expect(useWorkbench.getState().layout.left).toMatchObject({ open: true, active: view });
  expect(useWorkbench.getState().overlay).toBeNull();
});
