/** The thread connection report is named by purpose and loads its details only on intent. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ThreadGatewayDetails } from '@/features/gateways/ThreadGatewayStatus';
import { i18n, setLocale } from '@/i18n';
import { catalogue, tunnel } from '../../doubles/gateways';
import { show } from '../support';

const reads = vi.hoisted(() => vi.fn());
vi.mock('@/api/gateways', () => ({
  threadGatewayQuery: (id: string) => ({ queryKey: ['thread-connection', id], queryFn: async () => { reads(); return tunnel; } }),
  gatewayCatalogueQuery: { queryKey: ['connection-catalogue'], queryFn: async () => catalogue },
}));
afterEach(async () => { cleanup(); reads.mockClear(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`names the tool connection and keeps report recovery reachable (${locale})`, async () => {
  await setLocale(locale); show(<ThreadGatewayDetails threadId="thread" />);
  const disclosure = screen.getByText(locale === 'en' ? 'Tool connection' : '工具连接', { exact: true });
  expect(reads).not.toHaveBeenCalled();
  const details = disclosure.closest('details');
  if (details === null) throw new Error('The report opens from a disclosure');
  details.open = true; fireEvent(details, new Event('toggle'));
  await waitFor(() => expect(reads).toHaveBeenCalledTimes(1));
  expect(await screen.findByRole('button', { name: i18n.t('regionalGateways.refresh') })).toBeEnabled();
  expect(screen.getAllByText(locale === 'en' ? 'Tool connection' : '工具连接', { exact: false }).length).toBeGreaterThan(1);
});
