/** Configuration entrances and revisioned rate writes retain their actual native reader state. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodesApi from '@/api/nodes';
import type * as UsageApi from '@/api/usage';
import { ApiError } from '@/api/client';
import { AskModelsBlocked } from '@/features/ask/ModelAvailability';
import { KeptLayers } from '@/features/resources/hosts/KeptLayers';
import { RateCard } from '@/features/usage/RateCard';
import { RateDialog } from '@/features/usage/RateForm';
import { emptyDraft } from '@/features/usage/rates';
import type { Node, RateTable, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { askModels } from '../unit/ask-fixtures';
import { org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { detailHost } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ node: vi.fn<() => Promise<Node>>(), settings: vi.fn<() => Promise<ResolvedSetting[]>>(), write: vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(),
  nodeQuery: () => ({ queryKey: ['configuration-node'], queryFn: reads.node, retry: false }),
}));
vi.mock('@/api/usage', async (original) => ({ ...await original<typeof UsageApi>(),
  nodeSettingsQuery: () => ({ queryKey: ['configuration-rates-settings'], queryFn: reads.settings, retry: false }),
  ratesQuery: () => ({ queryKey: ['configuration-rates'], queryFn: (): Promise<RateTable> => Promise.resolve({ catalogue: { on: false, scale: '1', as_of: '' }, models: [] }) }),
  catalogueQuery: () => ({ queryKey: ['configuration-catalogue'], queryFn: () => Promise.resolve(null) }),
  setRateSetting: reads.write,
}));
beforeEach(() => { reads.node.mockReset().mockResolvedValue(org('root', 'acme', 'Acme', ADA)); reads.settings.mockReset().mockResolvedValue([]); reads.write.mockReset(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a failed Ask configuration entrance and retries its exact org reader (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); reads.node.mockRejectedValueOnce(new NetworkError('offline'));
    show(<AskModelsBlocked root="root" reply={askModels('root', { models: [], default_model: null, blocked_reason: 'empty', writable: true })} />);
    const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
    expect(screen.getByRole('button', { name: i18n.t('ask.configureModels') })).toHaveAttribute('aria-description', i18n.t('implicitAvailability.detailsUnavailable'));
    await user.click(retry);
    expect(await screen.findByRole('link', { name: i18n.t('ask.configureModels') })).toHaveAttribute('href', '/org/acme/settings/defaults?setting=ask.models');
    expect(reads.node).toHaveBeenCalledTimes(2);
  });
  it(`does not expose private org details through Ask configuration (${locale})`, async () => {
    await setLocale(locale); reads.node.mockRejectedValue(new ApiError({ type: '', title: 'Denied', status: 403, code: 'authz.denied', detail: 'Hidden organization', request_id: 'org-read' }, null));
    show(<AskModelsBlocked root="root" reply={askModels('root', { models: [], default_model: null, blocked_reason: 'empty', writable: true })} />);
    const configure = await screen.findByRole('button', { name: i18n.t('ask.configureModels') });
    expect(configure).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByText('Hidden organization')).not.toBeInTheDocument();
  });
  it(`keeps rate facts visible and guards every revisioned editor until settings retry succeeds (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); reads.settings.mockRejectedValueOnce(new NetworkError('offline'));
    show(<RateCard node={{ id: 'root', name: 'Acme', kind: 'org' }} manage />);
    expect(await screen.findByText(i18n.t('implicitAvailability.rateSettingsFailed'))).toBeVisible();
    const estimates = screen.getByRole('switch', { name: i18n.t('usage.rates.estimates') });
    expect(estimates).toBeDisabled();
    const scale = screen.getByRole('textbox', { name: i18n.t('usage.rates.scale') });
    expect(scale).toHaveAttribute('readonly');
    const form = scale.closest('form'); if (form === null) throw new Error('The existing scale editor keeps its form');
    fireEvent.change(scale, { target: { value: '2' } }); fireEvent.submit(form);
    expect(reads.write).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(screen.getByRole('switch', { name: i18n.t('usage.rates.estimates') })).toBeEnabled());
    expect(screen.getByRole('textbox', { name: i18n.t('usage.rates.scale') })).not.toHaveAttribute('readonly');
    expect(reads.settings).toHaveBeenCalledTimes(2);
  });
  it(`keeps a local rate draft while a current read failure guards Save (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    const view = show(<RateDialog node={{ id: 'root', name: 'Acme' }} mode="add" initial={emptyDraft('saved-pattern')} replacing={null} card={{}} revision={null} onClose={vi.fn()} />);
    const pattern = screen.getByRole('textbox', { name: i18n.t('usage.rates.form.pattern') });
    await user.clear(pattern); await user.type(pattern, 'local-pattern');
    view.rerender(<RateDialog node={{ id: 'root', name: 'Acme' }} mode="add" initial={emptyDraft('saved-pattern')} replacing={null} card={{}} revision={null} unavailableReason={i18n.t('implicitAvailability.rateSettingsFailed')} onClose={vi.fn()} />);
    expect(pattern).toHaveValue('local-pattern');
    const save = screen.getByRole('button', { name: i18n.t('usage.rates.form.save') });
    expect(save).toHaveAttribute('aria-disabled', 'true'); await user.click(save);
    expect(reads.write).not.toHaveBeenCalled();
  });
  it(`distinguishes empty kept files from an unreported private projection (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<KeptLayers host={{ ...detailHost, kept_layers: [] }} />);
    expect(screen.getByText(i18n.t('implicitAvailability.noKeptLayers'))).toBeVisible();
    view.rerender(<KeptLayers host={detailHost} />);
    expect(screen.getByText(i18n.t('implicitAvailability.detailsUnavailable'))).toBeVisible();
    expect(screen.queryByText(i18n.t('implicitAvailability.noKeptLayers'))).not.toBeInTheDocument();
  });
}
