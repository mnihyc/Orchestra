/** A thread directory's status follows the type of the existing entry. */
import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { DirectorySign } from '@/features/start/DirectorySign';
import type { FileStat } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ stat: vi.fn<() => Promise<FileStat>>() }));
vi.mock('@/api/workspace', async (original) => ({ ...await original<object>(), statFile: () => calls.stat() }));
beforeEach(() => { calls.stat.mockReset(); });
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['file', 'symlink', 'other'] as const)('does not call a %s a directory in ' + locale, async (type) => {
    await setLocale(locale);
    calls.stat.mockResolvedValue({ type } as FileStat);
    show(<DirectorySign environmentId="environment" directory="work" />);
    expect(await screen.findByText(i18n.t('start.where.directoryIsFile'))).toHaveAttribute('data-directory-sign', 'file');
  });
  it(`recognizes an existing directory and a missing path (${locale})`, async () => {
    await setLocale(locale);
    calls.stat.mockResolvedValueOnce({ type: 'dir' } as FileStat);
    const view = show(<DirectorySign environmentId="environment" directory="work" />);
    expect(await screen.findByText(i18n.t('start.where.directoryExists'))).toHaveAttribute('data-directory-sign', 'exists');
    view.unmount();
    calls.stat.mockRejectedValueOnce(new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404, title: 'Not found', detail: 'No entry', request_id: 'directory' }, null));
    show(<DirectorySign environmentId="environment" directory="new-work" />);
    expect(await screen.findByText(i18n.t('start.where.directoryNew'))).toHaveAttribute('data-directory-sign', 'new');
  });
}

const unavailableWords = {
  en: { unchecked: 'Directory not checked', offline: 'Environment offline', refused: 'Files access refused', checking: 'Checking directory…' },
  'zh-CN': { unchecked: '未检查目录', offline: '环境离线', refused: '文件访问被拒绝', checking: '正在检查目录…' },
};
function statRefusal(kind: 'offline' | 'refused') {
  return new ApiError({ type: 'orch:error', code: kind === 'offline' ? 'source.offline' : 'authz.denied', status: kind === 'offline' ? 503 : 403,
    title: 'Unavailable', detail: 'Hidden source and grant canary', request_id: 'directory-check' }, null);
}
for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['offline', 'refused'] as const)(`names an unchecked %s directory without leaking a private cause and can retry (${locale})`, async (kind) => {
    await setLocale(locale);
    calls.stat.mockRejectedValueOnce(statRefusal(kind)).mockResolvedValueOnce({ type: 'dir' } as FileStat);
    const view = show(<DirectorySign environmentId="environment" directory="work" />);
    expect(await screen.findByText(unavailableWords[locale].unchecked)).toBeVisible();
    expect(screen.getByText(unavailableWords[locale][kind])).toBeVisible();
    expect(view.container).not.toHaveTextContent('Hidden source and grant canary');
    expect(screen.queryByText(i18n.t('start.where.directoryNew'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('start.where.directoryExists'))).toBeVisible();
    expect(calls.stat).toHaveBeenCalledTimes(2);
  });
  it(`keeps loading separate from an absent directory (${locale})`, async () => {
    await setLocale(locale); calls.stat.mockImplementation(() => new Promise<FileStat>(() => undefined));
    show(<DirectorySign environmentId="environment" directory="work" />);
    expect(await screen.findByText(unavailableWords[locale].checking)).toBeVisible();
    expect(screen.queryByText(i18n.t('start.where.directoryNew'))).not.toBeInTheDocument();
    expect(screen.queryByText(unavailableWords[locale].unchecked)).not.toBeInTheDocument();
  });
  it(`does not keep an old existing sign after the current check fails (${locale})`, async () => {
    await setLocale(locale); calls.stat.mockResolvedValueOnce({ type: 'dir' } as FileStat);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    show(<QueryClientProvider client={cache}><DirectorySign environmentId="environment" directory="work" /></QueryClientProvider>);
    expect(await screen.findByText(i18n.t('start.where.directoryExists'))).toBeVisible();
    calls.stat.mockRejectedValue(statRefusal('offline'));
    await act(async () => { await cache.invalidateQueries({ queryKey: ['start', 'directory', 'environment', 'work'] }); });
    expect(await screen.findByText(unavailableWords[locale].unchecked)).toBeVisible();
    expect(screen.queryByText(i18n.t('start.where.directoryExists'))).not.toBeInTheDocument();
    cache.clear();
  });
}
