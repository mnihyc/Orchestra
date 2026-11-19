/** Work screens have useful words, visible select values and one account/connection explanation. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import type * as AuthApi from '@/api/auth';
import type * as UsageApi from '@/api/usage';
import type * as NodesApi from '@/api/nodes';
import type * as AccessApi from '@/api/access';
import type * as Live from '@/realtime/live';
import type * as PageModule from '@/shell/Page';
import { ApiError } from '@/api/client';
import { useCandidates } from '@/features/access/candidates';
import { NoAccess } from '@/features/access/NoAccess';
import { AuthFrame } from '@/features/auth/Frame';
import { ConnectionFailure } from '@/features/auth/ConnectionFailure';
import { Articles } from '@/features/help/HelpView';
import { SharedScreen } from '@/features/shared/Shared';
import { General } from '@/features/settings/org/General';
import { OrgSettings } from '@/features/settings/org/OrgSettings';
import { useFinder } from '@/features/finder/store';
import { UsageBoard } from '@/features/usage/Board';
import type { UsageAnalytics } from '@/generated/api-types';
import type { UsageParams } from '@/api/usage';
import type { Column } from '@/ui/Table';
import { makeGuide } from '@/help/guide';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { errorText } from '@/ui/errorText';
import { me, meta, org, userRef } from '../doubles/fixtures';
import { show } from './support';
import { retiredCopy } from './retired-copy';

const calls = vi.hoisted(() => ({ retry: vi.fn(), holders: vi.fn(), emptyShared: false }));
function home() { return org('00000000-0000-4000-8000-000000000001', 'acme', 'Acme', userRef(me(1, 'ada', 'Ada'))); }
function department() {
  const root = home(); const id = '00000000-0000-4000-8000-000000000002';
  return { ...root, id, name: 'Research', slug: 'research', address: 'acme.research', parent_id: root.id, depth: 1,
    path: [...root.path, { id, name: 'Research', kind: 'org' as const, redacted: false }] };
}
function holderRows() { return { items: [{ input: { kind: 'role', role: 'developer', at: '00000000-0000-4000-8000-000000000002' },
  holder: { kind: 'role', role: 'developer', count: 1, at: { id: '00000000-0000-4000-8000-000000000002', kind: 'org', name: 'Research', redacted: false } },
  place: 'Acme / Research', outside: false }], next_cursor: null }; }

vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  Link: ({ children, to }: { children: ReactNode; to: string }) => <a href={to}>{children}</a>,
  useRouter: () => ({ invalidate: calls.retry }),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(),
  metaQuery: { queryKey: ['reader-meta'], queryFn: () => Promise.resolve(meta({ name: 'Workshop' })) },
}));
vi.mock('@/api/shared', () => ({ sharedPageQuery: (cursor: string | null) => ({ queryKey: ['reader-shared', cursor], queryFn: () => Promise.resolve({ items: calls.emptyShared ? [] : [home()], next_cursor: null, partial: false }) }) }));
vi.mock('@/api/usage', async (original) => ({ ...await original<typeof UsageApi>(),
  usageQuery: (params: UsageParams) => ({ queryKey: ['reader-usage', params], queryFn: (): Promise<UsageAnalytics> => {
    // The populated breakdown needs real activity; a zero-use board correctly omits that picker.
    const figures = { turns: 1, tokens: { input_tokens: 10, cached_input_tokens: 0, output_tokens: 5, reasoning_tokens: 0, total_tokens: 15 }, credits: '0', estimated_credits: '0', unrated_tokens: 0 };
    return Promise.resolve({ scope: params.scope, from: params.from, to: params.to, bucket: params.bucket ?? 'day', depth: 'own', me: null, totals: figures,
      series: [{ at: `${params.from}T00:00:00Z`, figures, by: [] }], breakdowns: [{ by: 'model', more: 0, rows: [{ key: { model: 'private_model', model_display: 'Reasoner' }, figures, share: 1 }] }] });
  } }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  nodeQuery: (id: string) => ({ queryKey: ['reader-node', id], queryFn: () => Promise.resolve(id === department().id ? department() : home()) }),
  orgBySlugQuery: (slug: string) => ({ queryKey: ['reader-org', slug], queryFn: () => Promise.resolve(department()) }),
}));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof AccessApi>(),
  holderSearchQuery: (...args: unknown[]) => { calls.holders(...args); return { queryKey: ['reader-holders', ...args], queryFn: () => Promise.resolve(holderRows()) }; },
}));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@/shell/org', () => ({ selectOrg: vi.fn() }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useOrgTreeFeed: vi.fn() }));
vi.mock('@/shell/Page', async (original) => ({ ...await original<typeof PageModule>(), boxClasses: '', Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title, lead, actions }: { title: string; lead?: ReactNode; actions?: ReactNode }) => <header><h1>{title}</h1>{lead}{actions}</header>,
  PlaceMark: ({ children }: { children: ReactNode }) => <span>{children}</span>,
  Section: ({ title, lead, children }: { title: string; lead?: ReactNode; children: ReactNode }) => <section><h2>{title}</h2>{lead}{children}</section>,
}));
vi.mock('@/ui/Table', () => ({ Table: <Row extends { id: string },>({ rows, columns }: { rows: Row[]; columns: Column<Row>[] }) => <ul>{rows.map((row) => <li key={row.id}>{columns.map((column) => <div key={column.id}>{column.cell(row, new Set(columns.map((one) => one.id)))}</div>)}</li>)}</ul> }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabel: () => (node: { name?: string | null } | null | undefined) => node?.name ?? null, useKnownPlaces: () => new Map() }));
vi.mock('@/features/navigation/NodeDialogs', () => ({ openNodeDialog: vi.fn() }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn() }));
vi.mock('@/features/workbench/Narrow', () => ({ useShape: () => 'wide' }));

const appearance = useAppearance.getState();
const finderBefore = useFinder.getState();
const originalTheme = document.documentElement.getAttribute('data-theme');
afterEach(async () => {
  cleanup(); useAppearance.setState(appearance); useFinder.setState(finderBefore); calls.emptyShared = false; await setLocale('en'); vi.clearAllMocks();
  if (originalTheme === null) document.documentElement.removeAttribute('data-theme');
  else document.documentElement.dataset.theme = originalTheme;
});
function HolderPlaces() {
  const found = useCandidates('', home().id, { kinds: ['group', 'role'], placeId: 'actual-topic', thing: 'Repair login' });
  return found.placePicker;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`starts each actual select with a visible value and keeps its filter meaning in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const shared = show(<SharedScreen />);
    expect(await screen.findByRole('link', { name: 'Acme' })).toBeVisible();
    const select = screen.getByRole('combobox', { name: i18n.t('sharedWithMe.kind') });
    expect(select).toHaveTextContent(i18n.t('choiceCatalog.statusFilter.any.name'));
    expect(screen.getByText(i18n.t('sharedWithMe.kind'), { exact: true })).toBeVisible();
    await userEvent.click(select);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.placeKind.thread.name') }));
    expect(screen.getByText(i18n.t('sharedWithMe.noMatches'))).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('sharedWithMe.clearFilters') }));
    expect(select).toHaveTextContent(i18n.t('choiceCatalog.statusFilter.any.name'));
    expect(screen.getByRole('link', { name: 'Acme' })).toBeVisible();
    await userEvent.click(select);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.placeKind.thread.name') }));
    await userEvent.click(select);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.statusFilter.any.name') }));
    expect(screen.getByRole('link', { name: 'Acme' })).toBeVisible();
    shared.unmount();
    calls.emptyShared = true;
    const empty = show(<SharedScreen />);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('sharedWithMe.findPlaces') }));
    expect(useFinder.getState().open).toBe(true);
    empty.unmount();
  });

  it(`keeps the holder editor’s actual place value and scope in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const picker = show(<HolderPlaces />);
    const source = screen.getByRole('combobox', { name: i18n.t('access.candidates.place') });
    expect(source).toHaveValue(i18n.t('access.candidates.thisPlace', { place: 'Repair login' }));
    expect(screen.getAllByRole('combobox')).toHaveLength(1);
    await userEvent.click(source);
    await userEvent.click(await screen.findByRole('option', { name: 'Acme / Research' }));
    await waitFor(() => expect(calls.holders).toHaveBeenCalledWith('', '00000000-0000-4000-8000-000000000002', home().id, false));
    await userEvent.click(source);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('access.candidates.thisPlace', { place: 'Repair login' }) }));
    await waitFor(() => expect(calls.holders).toHaveBeenCalledWith('', 'actual-topic', home().id, false));
    expect(picker.container).not.toHaveTextContent('current');
  });

  it(`says a refusal and a connection cause once and retains the retry in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const error = new NetworkError('failed');
    const failure = show(<AuthFrame><ConnectionFailure error={error} onRetry={calls.retry} /></AuthFrame>);
    expect(await screen.findByRole('heading', { level: 1, name: 'Workshop' })).toBeVisible();
    const message = errorText(error, i18n.t, { locale }).message;
    expect(screen.getAllByText(message, { exact: true })).toHaveLength(1);
    expect(screen.getByText(i18n.t('shell.checkConnection'))).toBeVisible();
    expect(screen.queryByText(retiredCopy(locale, 'shell.unreachableLead'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(calls.retry).toHaveBeenCalledTimes(1);
    failure.unmount();
    const denied = new ApiError({ type: 'orch:error:authz.denied', status: 403, title: 'Forbidden', code: 'authz.denied', detail: 'Denied', request_id: 'request' }, null);
    show(<NoAccess error={denied} onRetry={calls.retry} />);
    expect(screen.getByRole('heading', { name: i18n.t('access.noAccess.title') })).toBeVisible();
    expect(screen.queryByText(retiredCopy(locale, 'access.noAccess.lead'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('ui.error.reviewAccess') }));
    expect(screen.getByRole('button', { name: i18n.t('access.noAccess.tryAgain') })).toBeEnabled();
  });

  it(`orders articles for first use without section counts and keeps the real address in General in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const names = locale === 'en' ? ['Getting started', 'Workflows', 'Resources'] : ['开始使用', '工作流程', '资源'];
    const guide = makeGuide({ 'resources.md': `# ${names[2]}\n\n## One\n\n## Two`, 'getting-started.md': `# ${names[0]}`, 'workflows.md': `# ${names[1]}` });
    const open = vi.fn();
    const articles = show(<Articles guide={guide} onArticle={open} />);
    expect(screen.getAllByRole('button').map((button) => button.textContent)).toEqual(names);
    expect(screen.queryByText(/2 sections|2 节/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: names[0] }));
    expect(open).toHaveBeenCalledWith('getting-started');
    articles.unmount();
    const general = show(<General node={home()} />);
    expect(screen.getByLabelText(i18n.t('settings.general.shortName'))).toHaveValue('acme');
    expect(within(general.container).queryByText(retiredCopy(locale, 'settings.general.dangerLead'))).not.toBeInTheDocument();
    general.unmount();
    const settings = show(<OrgSettings slug="acme.research" section="general" />);
    expect(await screen.findByLabelText(i18n.t('settings.general.shortName'))).toHaveValue('research');
    expect(settings.container.querySelector('header')).not.toHaveTextContent('acme.research');
    expect(screen.queryByText(retiredCopy(locale, 'settings.org.lead'))).not.toBeInTheDocument();
  });

  it(`labels the overview's This week and breakdown selectors in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    show(<UsageBoard nodeId={home().id} name="Acme" />);
    const period = screen.getByRole('combobox', { name: i18n.t('usage.board.period') });
    expect(screen.getByText(i18n.t('usage.board.period'), { exact: true })).toBeVisible();
    expect(period).toHaveTextContent(i18n.t('usage.board.periods.week.name'));
    const breakdown = await screen.findByRole('combobox', { name: i18n.t('usage.board.divideBy') });
    expect(screen.getByText(i18n.t('usage.board.divideBy'), { exact: true })).toBeVisible();
    expect(breakdown).toHaveTextContent(i18n.t('usagePerf.choices.splits.model.name'));
    expect(screen.getByText(/Reasoner/)).toBeVisible();
    expect(screen.queryByText('private_model')).not.toBeInTheDocument();
  });
}
