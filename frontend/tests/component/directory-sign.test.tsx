/** A thread directory's status follows the type of the existing entry. */
import { screen } from '@testing-library/react';
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
