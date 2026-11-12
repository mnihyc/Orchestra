/** The registered thread Usage view never mistakes a failed provider read for missing View permission. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as Resources from '@/api/resources';
import { ThreadUsage } from '@/features/thread/UsageView';
import type { Provider, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
const read = vi.hoisted(() => vi.fn<() => Promise<Provider>>());
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof Resources>(), providerQuery: () => ({ queryKey: ['thread-provider-observations'], queryFn: read, retry: false }) }));
vi.mock('@/api/provider-plan', () => ({ threadLedgerQuery: () => ({ queryKey: ['thread-ledger-read'], queryFn: () => Promise.resolve({ totals: {} }) }) }));
vi.mock('@/features/resources/providers/Ledger', () => ({ LedgerFigures: () => <span>Ordinary ledger</span> }));
vi.mock('@/features/resources/providers/PlanPanel', () => ({ PlanPanel: () => <span>Ordinary provider plan</span> }));
vi.mock('@/features/usage/context/ThreadComposition', () => ({ ThreadComposition: () => null }));
vi.mock('@/features/usage/ThreadUsage', () => ({ meteringOf: () => undefined }));
vi.mock('@/features/usage/Limits', () => ({ Limits: () => null }));
vi.mock('@/features/usage/PerformancePlaces', () => ({ useProviderHealth: () => undefined }));
const thread = { id: 'thread', created_at: '2026-11-01T00:00:00Z', name: 'Thread', blocked: null,
  binding: { provider: { id: 'provider', name: 'Known provider', status: 'ready' } }, access: { permissions: [] } } as unknown as Thread;
beforeEach(() => { read.mockReset().mockResolvedValue(planProvider()); });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a failed provider observation read separate from View refusal and retries (${locale})`, async () => {
    await setLocale(locale); read.mockRejectedValueOnce(new NetworkError('offline'));
    const user = userEvent.setup(); show(<ThreadUsage thread={thread} />);
    expect(await screen.findByText(i18n.t('implicitAvailability.usageFailed'))).toBeVisible();
    expect(screen.queryByText(i18n.t('providerPlan.viewMissing'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText('Ordinary provider plan')).toBeVisible();
    expect(read).toHaveBeenCalledTimes(2);
  });
  it(`keeps a current private provider refusal and its hidden details out of observations (${locale})`, async () => {
    await setLocale(locale);
    read.mockRejectedValue(new ApiError({ type: '', title: 'Denied', status: 403, code: 'authz.denied', detail: 'Hidden provider details', request_id: 'usage-provider' }, null));
    show(<ThreadUsage thread={thread} />);
    expect(await screen.findByText(i18n.t('providerPlan.viewMissing'))).toBeVisible();
    expect(screen.queryByText('Hidden provider details')).not.toBeInTheDocument();
    expect(screen.queryByText('Ordinary provider plan')).not.toBeInTheDocument();
  });
}
