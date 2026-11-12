/** Pending requests distinguish the native Manage door, failed reads and a genuinely empty list. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as PortsApi from '@/api/ports';
import { PortRequests } from '@/features/ports/Request';
import type { PortRequest } from '@/features/ports/presentation';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { portsEnvironment } from './ports-fixtures';
import { show } from './support';

const read = vi.hoisted(() => vi.fn<() => Promise<PortRequest[]>>());
vi.mock('@/api/ports', async (original) => ({ ...await original<typeof PortsApi>(), portRequestsQuery: () => ({ queryKey: ['port-request-read'], queryFn: read, retry: false }) }));
beforeEach(() => { read.mockReset().mockResolvedValue([]); });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps missing Manage explicit without reading private requests (${locale})`, async () => {
    await setLocale(locale);
    show(<PortRequests environment={{ ...portsEnvironment, access: { ...portsEnvironment.access, permissions: ['ports.view'] } }} onChanged={vi.fn()} />);
    expect(screen.getByText(i18n.t('implicitAvailability.portsManage'))).toBeVisible();
    expect(read).not.toHaveBeenCalled();
  });
  it(`retries a failed request read without claiming it is an empty list (${locale})`, async () => {
    await setLocale(locale); read.mockRejectedValueOnce(new NetworkError('offline')).mockResolvedValue([]);
    const user = userEvent.setup();
    show(<PortRequests environment={{ ...portsEnvironment, access: { ...portsEnvironment.access, permissions: ['ports.view', 'ports.manage'] } }} onChanged={vi.fn()} />);
    await screen.findByRole('alert');
    expect(screen.queryByText(i18n.t('implicitAvailability.noPortRequests'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('implicitAvailability.noPortRequests'))).toBeVisible();
    expect(read).toHaveBeenCalledTimes(2);
  });
}
