/** The inspector reports the resolved option, including unknown and failed reads. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { PersonalReads } from '@/features/inspector/PersonalReads';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

const source = vi.hoisted(() => ({ rows: [] as ResolvedSetting[], failed: false, pending: false, release: null as ((rows: ResolvedSetting[]) => void) | null, read: vi.fn() }));
vi.mock('@/api/settings', () => ({
  settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => {
    source.read(id);
    if (source.pending) return new Promise<ResolvedSetting[]>((resolve) => { source.release = resolve; });
    if (source.failed) throw new Error('Current settings refused');
    return Promise.resolve(source.rows);
  }, staleTime: Infinity }),
}));
// The existing guide has its own opening tests; retain its actual article identity here.
vi.mock('@/features/help/ArticleGuideLink', () => ({ ArticleGuideLink: ({ article, title }: { article: string; title: string }) => <a href={`/help/${article}`}>{title}</a> }));

function row(value: boolean): ResolvedSetting {
  return { key: 'orch_tools.owner_reads', value, own: null, writable: false, locked_by: { scope: 'org', node: null }, source: { scope: 'org', node: null }, parts: [] };
}
function draw(id = 'thread-one') {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={cache}><PersonalReads threadId={id} /></QueryClientProvider>);
  return { cache, ...view };
}

for (const locale of ['en', 'zh-CN'] as const) describe(`personal read inspector in ${locale}`, () => {
  beforeEach(async () => { await setLocale(locale); source.rows = []; source.failed = false; source.pending = false; source.release = null; source.read.mockClear(); });

  it('keeps an inherited, locked On visible and names the shared-read alternative', async () => {
    source.rows = [row(true)]; draw();
    await screen.findByText(i18n.t('thread.audience.reads.on'));
    expect(screen.getByTestId('personal-read-state')).toHaveTextContent(i18n.t('thread.audience.reads.shared'));
    expect(screen.getByRole('link', { name: i18n.t('thread.audience.reads.guide') })).toHaveAttribute('href', '/help/personal-read-tools');
    expect(screen.queryByText(i18n.t('thread.audience.reads.off'))).not.toBeInTheDocument();
  });

  it('follows a real option change through the current thread settings cache', async () => {
    source.rows = [row(true)]; const { cache } = draw();
    await screen.findByText(i18n.t('thread.audience.reads.on'));
    source.rows = [row(false)];
    await act(() => cache.invalidateQueries({ queryKey: ['settings', 'thread-one'] }));
    await screen.findByText(i18n.t('thread.audience.reads.off'));
    expect(screen.queryByText(i18n.t('thread.audience.reads.on'))).not.toBeInTheDocument();
  });

  it('keeps a missing option unknown rather than calling it Off', async () => {
    draw(); await screen.findByText(i18n.t('thread.audience.reads.unavailable'));
    expect(screen.queryByText(i18n.t('thread.audience.reads.off'))).not.toBeInTheDocument();
  });

  it('withdraws stale On after a refused read and retries the actual settings', async () => {
    source.rows = [row(true)]; const { cache } = draw();
    await screen.findByText(i18n.t('thread.audience.reads.on'));
    source.failed = true;
    await act(() => cache.invalidateQueries({ queryKey: ['settings', 'thread-one'] }));
    await screen.findByText(i18n.t('thread.audience.reads.unavailable'));
    expect(screen.queryByText(i18n.t('thread.audience.reads.on'))).not.toBeInTheDocument();
    source.failed = false; source.rows = [row(false)];
    await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await screen.findByText(i18n.t('thread.audience.reads.off'));
    expect(source.read).toHaveBeenLastCalledWith('thread-one');
  });

  it('does not borrow the previous thread’s option while a new target loads', async () => {
    source.rows = [row(true)]; const view = draw();
    await screen.findByText(i18n.t('thread.audience.reads.on'));
    source.pending = true;
    view.rerender(<QueryClientProvider client={view.cache}><PersonalReads threadId="thread-two" /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByTestId('personal-read-state')).toHaveTextContent(i18n.t('thread.audience.reads.loading')));
    expect(screen.queryByText(i18n.t('thread.audience.reads.on'))).not.toBeInTheDocument();
    await act(() => Promise.resolve(source.release?.([row(false)])));
    await screen.findByText(i18n.t('thread.audience.reads.off'));
  });
});
