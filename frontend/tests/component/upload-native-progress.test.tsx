/** Files reuse the shared native job line and keep unconfirmed cancellation visible in both languages. */
import { cleanup, fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { UploadChips } from '@/features/composer/Attachments';
import type { Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import type { QueuedUpload } from '@/lib/useUploads';
import { show } from './support';

vi.mock('@/features/composer/UploadHelp', () => ({ UploadHelp: () => null }));

function job(): Job {
  const now = new Date().toISOString();
  return { id: 'native-write', kind: 'delivery', state: 'running', node_id: null, environment_id: 'workspace', provider_id: null, batch_id: null,
    result: null, error: null, log: null, created_by: null, created_at: now, started_at: now, finished_at: null,
    progress: { phase: 'files', phases: [{ name: 'files', state: 'running', weight: 100 }], percent: 12,
      files_done: 0, files_total: 1, bytes_done: 512, bytes_total: 4096, rate_bps: 512, eta_seconds: 7, message: null,
      activity: { updated_at: now, stall_after_seconds: 30, directories_done: null, stopping: false, waiting_for: null } } };
}
function queue(row: Partial<QueuedUpload> = {}) {
  const value: QueuedUpload = { id: 'upload', file: new File([new Uint8Array(4096)], 'report.bin'), received: 4096, phase: 'writing', ...row };
  return { rows: [value],
    remove: vi.fn<(id: string) => void>(), retry: vi.fn<(id: string) => void>() };
}

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows actual copy counters and no premature retry control in ${locale}`, async () => {
    await setLocale(locale);
    const value = queue({ work: job() });
    const view = show(<UploadChips queue={value} />);
    expect(view.container.querySelector('[data-job-line]')).toHaveTextContent(locale === 'en' ? 'Copying' : '复制中');
    expect(view.container.querySelector('[data-job-line]')).toHaveTextContent('512');
    expect(view.container.querySelector('[data-job-elapsed]')).not.toBeNull();
    expect(screen.getByRole('button', { name: i18n.t('uploads.cancel', { name: 'report.bin' }) })).toBeEnabled();
    expect(screen.queryByRole('button', { name: i18n.t('uploads.retry', { name: 'report.bin' }) })).not.toBeInTheDocument();
  });

  it(`waits for the admission reply before claiming the machine has started in ${locale}`, async () => {
    await setLocale(locale);
    const view = show(<UploadChips queue={queue()} />);
    expect(view.container).toHaveTextContent(locale === 'en' ? 'Waiting' : '等待中');
    expect(view.container).not.toHaveTextContent(locale === 'en' ? 'Copying' : '复制中');
  });

  it(`keeps an unknown native stop visible and does not claim Cancelled in ${locale}`, async () => {
    await setLocale(locale);
    const view = show(<UploadChips queue={queue({ phase: 'stopping' })} />);
    expect(view.container).toHaveTextContent(locale === 'en' ? 'Stopping…' : '正在停止…');
    expect(view.container).not.toHaveTextContent(locale === 'en' ? 'Cancelled' : '已取消');
    expect(screen.getByRole('button', { name: i18n.t('uploads.cancel', { name: 'report.bin' }) })).toBeDisabled();
  });

  it(`uses the job's stop allowance and retries a failed stop without restarting its write in ${locale}`, async () => {
    await setLocale(locale);
    const active = job();
    active.progress.activity = { ...active.progress.activity!, stopping: true, waiting_for: { type: 'machine', name: 'lab-host' }, updated_at: new Date(Date.now() - 31_000).toISOString() };
    const value = queue({ phase: 'stopping', work: active, error: new Error('Lost connection') });
    const view = show(<UploadChips queue={value} />);
    expect(view.container.querySelector('[data-job-line]')).toHaveTextContent(locale === 'en' ? 'Still stopping on lab-host' : '仍在停止 lab-host 上的工作');
    expect(screen.getByRole('alert')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(value.remove).toHaveBeenCalledExactlyOnceWith('upload');
    expect(value.retry).not.toHaveBeenCalled();
    expect(screen.getByText('report.bin')).toBeVisible();
  });
}
