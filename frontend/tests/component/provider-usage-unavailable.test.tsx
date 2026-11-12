/** Unavailable usage remains visible without disclosing retained facts or starting a history read. */
import { act, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { Plan } from '@/features/usage/Plan';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const transport = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: transport.get, POST: transport.post } };
});
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
// This case concerns admission and the section's visible state; chart rendering has its own cases.
vi.mock('@/features/usage/PlanChart', () => ({ PlanChart: () => null }));
vi.mock('@/features/usage/CreditChart', () => ({ CreditChart: () => null, creditGroups: () => [] }));

const span = { from: '2026-11-01', to: '2026-11-03', days: 3, range: 'custom' as const };
beforeEach(() => {
  transport.get.mockReset(); transport.post.mockReset();
  transport.get.mockResolvedValue({ data: { items: [], next_cursor: null }, response: new Response() });
});
afterEach(async () => { document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`explains private, unsupported and unread usage without borrowing history access ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const provider = planProvider();
    const source = provider.sources.facts.descriptor;
    if (source === null) throw new Error('The source fixture must have a declaration');
    source.title = 'Retained private source';
    provider.facts.windows[0]!.label = 'Retained private window';
    provider.access.permissions = ['provider.manage'];
    const render = () => <Plan provider={structuredClone(provider)} span={span} mode="current" bucket="day" onMode={vi.fn()} onBucket={vi.fn()} />;
    const view = show(render());
    expect(screen.getByText(i18n.t('providerCredits.unavailableHere'))).toBeVisible();
    expect(screen.queryByText('Retained private source')).not.toBeInTheDocument();
    expect(screen.queryByText('Retained private window')).not.toBeInTheDocument();
    expect(view.container.querySelector('[data-provider-current]')).not.toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    await act(async () => { await Promise.resolve(); });
    expect(transport.get).not.toHaveBeenCalled();
    expect(transport.post).not.toHaveBeenCalled();

    provider.access.permissions = ['provider.view'];
    for (const member of ['windows', 'balances', 'spend', 'extra_usage'] as const) provider.facts_state[member] = 'unsupported';
    view.rerender(render());
    expect(screen.getByText(i18n.t('providerCredits.historyUnsupported'))).toBeVisible();
    expect(screen.queryByText(i18n.t('providerCredits.unavailableHere'))).not.toBeInTheDocument();
    expect(screen.queryByText('Retained private window')).not.toBeInTheDocument();
    expect(transport.get).not.toHaveBeenCalled();

    provider.facts_state.spend = { unsupported_because: { reason: 'needs_admin_key' } };
    view.rerender(render());
    expect(screen.getByText(i18n.t('providerSources.needsAdminKey'))).toBeVisible();
    expect(screen.queryByText(i18n.t('providerCredits.historyUnsupported'))).not.toBeInTheDocument();
    expect(transport.get).not.toHaveBeenCalled();

    provider.facts_state.windows = 'not_read_yet';
    view.rerender(render());
    expect(view.container.querySelector('[data-current-member="windows"]')).toHaveTextContent(i18n.t('providerDetails.notReadOnce'));
    expect(screen.queryByText(i18n.t('providerCredits.historyUnsupported'))).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('providers.overview.refresh') })).toBeEnabled();
    await act(async () => { await Promise.resolve(); });
    expect(transport.get).not.toHaveBeenCalled();

    provider.facts_state.windows = 'told';
    view.rerender(render());
    await waitFor(() => expect(transport.get).toHaveBeenCalledTimes(1));
    expect(transport.get.mock.calls[0]?.[0]).toBe('/providers/{id}/plan');
    expect(transport.post).not.toHaveBeenCalled();
  });
}
