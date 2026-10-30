/** Narrow threads keep their composer space; the drawer and finder remain available without bottom tabs. */
import { screen } from '@testing-library/react';
import type * as Router from '@tanstack/react-router';
import type { ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MobileNavigation } from '@/shell/MobileNavigation';
import { i18n } from '@/i18n';
import { show } from './support';

const route = vi.hoisted(() => ({ path: '/thread/thread-1' }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useRouterState: ({ select }: { select: (state: { location: { pathname: string } }) => unknown }) => select({ location: { pathname: route.path } }),
  Link: ({ to, children, className, 'aria-label': label }: { to: string; children: ReactNode; className?: string; 'aria-label'?: string }) => <a href={to} className={className} aria-label={label}>{children}</a>,
}));
vi.mock('@/features/workbench/NarrowViews', () => ({ useNarrowWorkbench: () => ({ choices: [{ id: 'left:navigation', name: i18n.t('views.navigation.title'), description: '', icon: null, active: false, onSelect: vi.fn() }], main: <h1>Conversation</h1>, primary: null, mainTabs: [], primaryGroup: null, selectMain: vi.fn(), sheets: null, openNavigation: vi.fn() }) }));
vi.mock('@/shell/TitleBar', () => ({ ScreenTitleBar: ({ before, children }: { before: ReactNode; children: ReactNode }) => <header>{before}{children}</header> }));
vi.mock('@/shell/Banners', () => ({ Banners: () => null }));
vi.mock('@/shell/Rail', () => ({ useAccountMenu: () => [] }));
vi.mock('@/api/nodes', () => ({ orgsQuery: { queryKey: ['mobile-test-orgs'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/features/inbox/Counters', () => ({ useInboxCounters: () => ({ unread: 0, approvalsPending: 0 }), useCounterWords: () => ({ unread: null, approvals: null }), CountBadge: () => null }));
beforeEach(() => { route.path = '/thread/thread-1'; });

it('removes thread bottom tabs while retaining the screen, navigation door and finder', () => {
  show(<MobileNavigation onShortcuts={vi.fn()} />);
  expect(screen.getByRole('heading', { name: 'Conversation' })).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('views.navigation.title') })).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('finder.open') })).toBeVisible();
  expect(screen.queryByRole('navigation', { name: i18n.t('shell.rail.label') })).not.toBeInTheDocument();
});

it('keeps bottom navigation on places outside the thread', () => {
  route.path = '/topic/login';
  show(<MobileNavigation onShortcuts={vi.fn()} />);
  expect(screen.getByRole('navigation', { name: i18n.t('shell.rail.label') })).toBeVisible();
});
