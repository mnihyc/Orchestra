/** Actual query retries stay local to the view that is waiting for those rows. */
import { QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LiveQueryClient, queryClient } from '@/api/query';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError, READ_RETRY_PAUSES_MS } from '@/lib/requests';
import { SkeletonRows } from '@/ui/Skeleton';
import { show } from './support';

let client: LiveQueryClient;
beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  client = new LiveQueryClient({ defaultOptions: queryClient.getDefaultOptions() });
});
afterEach(async () => {
  cleanup(); client.clear(); vi.useRealTimers(); vi.restoreAllMocks();
  useAppearance.setState({ locale: 'en' }); await setLocale('en');
});
const settle = () => act(async () => { await vi.advanceTimersByTimeAsync(1); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the first failure, all configured retries and explicit recovery in ${locale}`, async () => {
    useAppearance.setState({ locale }); await setLocale(locale);
    let available = false;
    const read = vi.fn(() => available ? Promise.resolve('Loaded rows') : Promise.reject(new NetworkError('failed')));
    const other = () => new Promise<string>(() => { /* An independent read is still on its first attempt. */ });
    function View() {
      const own = useQuery({ queryKey: ['own'], queryFn: read });
      const pending = useQuery({ queryKey: ['other'], queryFn: other });
      return <><section aria-label="Own">{own.data ?? <SkeletonRows read={own} rows={2} />}</section>
        <section aria-label="Other"><SkeletonRows read={pending} rows={1} /></section></>;
    }
    show(<QueryClientProvider client={client}><View /></QueryClientProvider>);
    const own = within(screen.getByRole('region', { name: 'Own' }));
    const pending = within(screen.getByRole('region', { name: 'Other' }));
    expect(own.getByText(i18n.t('common.loading'))).toHaveClass('sr-only');
    await settle();
    expect(read).toHaveBeenCalledTimes(1);
    expect(own.getByText(i18n.t('ui.read.retrying'))).toBeVisible();
    expect(pending.queryByText(i18n.t('ui.read.retrying'))).not.toBeInTheDocument();
    for (let attempt = 0; attempt < READ_RETRY_PAUSES_MS.length; attempt += 1) {
      await act(async () => { await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[attempt]!); });
      await settle();
      expect(read).toHaveBeenCalledTimes(attempt + 2);
    }
    expect(own.getByText(i18n.t('ui.read.failed'))).toBeVisible();
    expect(own.queryByText(i18n.t('ui.read.retrying'))).not.toBeInTheDocument();
    expect(pending.getByText(i18n.t('common.loading'))).toHaveClass('sr-only');
    available = true;
    fireEvent.click(own.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await settle();
    expect(own.getByText('Loaded rows')).toBeVisible();
    expect(read).toHaveBeenCalledTimes(READ_RETRY_PAUSES_MS.length + 2);
  });
}

it('does not label a background refresh of retained content as a first load', () => {
  show(<SkeletonRows read={{ data: ['kept'], failureCount: 2, isFetching: true }} rows={1} />);
  expect(screen.queryByText(i18n.t('ui.read.retrying'))).not.toBeInTheDocument();
});
