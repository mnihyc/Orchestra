/** Header read failures and empty native host facts remain distinct from a private projection. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import { ApiError } from '@/api/client';
import { ResourceSharingSummary } from '@/features/resources/SharingSummary';
import { Drivers, Storages } from '@/features/resources/hosts/Host';
import type { PermissionPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { detailHome, detailHost } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ defaults: vi.fn<() => Promise<PermissionPage>>(), catalogue: vi.fn<() => Promise<typeof CATALOGUE.reply>>() }));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof Access>(),
  defaultsQuery: () => ({ queryKey: ['availability-defaults'], initialData: pageFor('template'), queryFn: reads.defaults, retry: false }),
  permissionsQuery: { queryKey: ['availability-permissions'], initialData: () => CATALOGUE.reply, queryFn: reads.catalogue, retry: false },
}));
beforeEach(() => {
  reads.defaults.mockReset().mockResolvedValue(pageFor('template'));
  reads.catalogue.mockReset().mockResolvedValue(CATALOGUE.reply);
});
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`withdraws an old audience and retries only the failed sharing reader (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    reads.defaults.mockRejectedValueOnce(new NetworkError('offline'));
    const view = show(<ResourceSharingSummary home={detailHome} kinds={['template']} />);
    expect(await screen.findByText(i18n.t('implicitAvailability.sharingFailed'))).toBeVisible();
    expect(view.container.querySelector('[data-resource-sharing]')).toBeNull();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(view.container.querySelector('[data-resource-sharing]')).not.toBeNull());
    expect(reads.defaults).toHaveBeenCalledTimes(2);
    expect(reads.catalogue).toHaveBeenCalledTimes(1);
  });

  it(`keeps a refused sharing projection nameless and does not reuse the old audience (${locale})`, async () => {
    await setLocale(locale);
    reads.defaults.mockRejectedValue(new ApiError({ type: '', title: 'Denied', status: 403, code: 'authz.denied', detail: 'Private audience detail', request_id: 'private-request' }, null));
    const view = show(<ResourceSharingSummary home={detailHome} kinds={['template']} />);
    expect(await screen.findByText(i18n.t('implicitAvailability.sharingUnavailable'))).toBeVisible();
    expect(view.container.querySelector('[data-resource-sharing]')).toBeNull();
    expect(screen.queryByText('Private audience detail')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
  });

  it(`names truly empty host facts without claiming unsupported drivers or storage (${locale})`, async () => {
    await setLocale(locale);
    const host = { ...detailHost, drivers: [], storages: [] };
    show(<><Drivers host={host} /><Storages host={host} /></>);
    expect(screen.getByRole('heading', { name: i18n.t('hosts.drivers.title') })).toBeVisible();
    expect(screen.getByRole('heading', { name: i18n.t('hosts.storages.title') })).toBeVisible();
    expect(screen.getByText(i18n.t('implicitAvailability.noHostDrivers'))).toBeVisible();
    expect(screen.getByText(i18n.t('implicitAvailability.noHostStorages'))).toBeVisible();
  });

  it(`does not describe private host projections as an empty catalogue (${locale})`, async () => {
    await setLocale(locale);
    const host = { ...detailHost, drivers: [], storages: [], access: { ...detailHost.access, permissions: [] } };
    show(<><Drivers host={host} /><Storages host={host} /></>);
    expect(screen.getAllByText(i18n.t('implicitAvailability.detailsUnavailable'))).toHaveLength(2);
    expect(screen.queryByText(i18n.t('implicitAvailability.noHostDrivers'))).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('implicitAvailability.noHostStorages'))).not.toBeInTheDocument();
  });
}
