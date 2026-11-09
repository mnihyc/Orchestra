/** Current human system reach controls the same report views; an old system snapshot never becomes personal usage. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Usage from '@/api/usage';
import type * as Nodes from '@/api/nodes';
import type * as Resources from '@/api/resources';
import type { UsageAnalytics } from '@/generated/api-types';
import { Report } from '@/features/usage/Report';
import { i18n, setLocale } from '@/i18n';
import { me } from '../doubles/fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ admin: true, mode: true, figures: vi.fn<(params: Usage.UsageParams) => void>(), node: vi.fn(), provider: vi.fn(), requests: vi.fn<(scope: { kind: string }) => void>() }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), meQuery: { queryKey: ['system-report-me'], queryFn: () => Promise.resolve(me(1, 'admin', 'Administrator', { is_system_admin: calls.admin })) } }));
vi.mock('@/features/access/AdminMode', () => ({ useAdminMode: () => ({ data: { as_admin: calls.mode }, isPending: false, isError: false, error: null, refetch: () => Promise.resolve() }), AdminModeEnableAction: () => <button type="button">Enable system reach</button> }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), nodeQuery: (id: string) => ({ queryKey: ['system-report-node', id], queryFn: () => { calls.node(); return Promise.resolve(null); } }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof Resources>(), providerQuery: (id: string) => ({ queryKey: ['system-report-provider', id], queryFn: () => { calls.provider(); return Promise.resolve(null); } }) }));
vi.mock('@/features/usage/Requests', () => ({ Requests: ({ scope }: { scope: { kind: string } }) => { calls.requests(scope); return <p>System request snapshot</p>; } }));
vi.mock('@/features/usage/Chart', () => ({ UsageChart: () => null }));
vi.mock('@/features/usage/Limits', () => ({ Limits: ({ of }: { of: { kind: string } }) => <span data-testid="scope-limits">{of.kind}</span> }));
vi.mock('@/features/usage/Export', () => ({ Export: () => <button type="button">Export current scope</button> }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/usage', async (original) => {
  const actual = await original<typeof Usage>();
  return { ...actual, usageQuery: (params: Usage.UsageParams) => ({ ...actual.usageQuery(params), queryFn: (): Promise<UsageAnalytics> => {
    calls.figures(params); const totals = { turns: 0, tokens: { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 }, credits: '0', estimated_credits: '0', unrated_tokens: 0 };
    return Promise.resolve({ scope: params.scope, depth: 'reports', from: params.from, to: params.to, bucket: 'day', totals, me: null, series: [], breakdowns: params.breakdowns.map((by) => ({ by, more: 0, rows: [] })) });
  } }), eventsQuery: (params: Usage.EventParams) => ({ ...actual.eventsQuery(params), queryFn: () => Promise.resolve({ items: [], next_cursor: null }) }) };
});
beforeEach(() => { vi.clearAllMocks(); calls.admin = true; calls.mode = true; });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`the System report asks its native aggregate without looking up a node or provider (${locale})`, async () => {
    await setLocale(locale); show(<Report scope={{ kind: 'system' }} search={{ scope: 'system', view: 'usage' }} onSearch={() => undefined} />);
    await waitFor(() => expect(calls.figures).toHaveBeenCalled()); expect(calls.figures.mock.calls.every(([params]) => params.scope === 'system')).toBe(true);
    expect(screen.getByRole('combobox', { name: i18n.t('usage.scope.label') })).toHaveValue(i18n.t('usage.scope.system'));
    expect(calls.node).not.toHaveBeenCalled(); expect(calls.provider).not.toHaveBeenCalled(); expect(screen.getByTestId('scope-limits')).toHaveTextContent('system');
  });
  it(`turning reach off removes the System requests view and export immediately (${locale})`, async () => {
    await setLocale(locale); const view = show(<Report scope={{ kind: 'system' }} search={{ scope: 'system', view: 'requests' }} onSearch={() => undefined} />);
    await screen.findByText('System request snapshot'); expect(calls.requests).toHaveBeenCalledWith({ kind: 'system' });
    calls.mode = false; view.rerender(<Report scope={{ kind: 'system' }} search={{ scope: 'system', view: 'requests' }} onSearch={() => undefined} />);
    expect(screen.queryByText('System request snapshot')).not.toBeInTheDocument(); expect(screen.queryByRole('button', { name: 'Export current scope' })).not.toBeInTheDocument(); expect(screen.getByRole('button', { name: 'Enable system reach' })).toBeInTheDocument();
    expect(calls.figures).not.toHaveBeenCalled();
  });
  it(`a non-system administrator's deep link stays explicit and reads no system report (${locale})`, async () => {
    await setLocale(locale); calls.admin = false; show(<Report scope={{ kind: 'system' }} search={{ scope: 'system' }} onSearch={() => undefined} />);
    await screen.findByText(i18n.t('usage.scope.adminRequired')); expect(calls.figures).not.toHaveBeenCalled(); expect(calls.requests).not.toHaveBeenCalled();
    expect(screen.getByRole('combobox', { name: i18n.t('usage.scope.label') })).toHaveValue(i18n.t('usage.scope.system'));
  });
}
