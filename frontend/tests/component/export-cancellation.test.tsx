/** A followed archive acknowledges a server-confirmed cancellation exactly once. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { jobQuery } from '@/api/exports';
import { useExportWatch, useWatched, watchExport } from '@/features/export/jobs';
import type { Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

function Watch() { useExportWatch(); return null; }
const job: Job = { id: 'archive-job', kind: 'export', state: 'running', batch_id: null, environment_id: null, provider_id: null, node_id: null, created_by: null,
  error: null, result: null, log: null, created_at: '2026-10-26T12:00:00Z', started_at: '2026-10-26T12:00:00Z', finished_at: null,
  progress: { phase: 'write', phases: [], percent: 40, bytes_done: 400, bytes_total: 1000, files_done: 1, files_total: 2, eta_seconds: null, message: null, rate_bps: null } };

afterEach(async () => {
  for (const button of screen.queryAllByRole('button', { name: i18n.t('common.dismiss') })) fireEvent.click(button);
  cleanup(); useWatched.setState({ items: [] }); sessionStorage.removeItem('orch.exports.watched'); await setLocale('en');
});

it.each(['en', 'zh-CN'] as const)('announces an export cancellation and its place in %s', async (locale) => {
  await setLocale(locale);
  watchExport(job, 'Team work');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const cancelled: Job = { ...job, state: 'cancelled', finished_at: '2026-10-26T12:01:00Z' };
  cache.setQueryData(jobQuery(job.id).queryKey, cancelled);
  const view = show(<QueryClientProvider client={cache}><Watch /></QueryClientProvider>);
  try {
    expect(await screen.findByText(i18n.t('export.job.cancelled'), { exact: true })).toBeVisible();
    expect(screen.getByText('Team work', { exact: true })).toBeVisible();
    expect(useWatched.getState().items).toEqual([]);
    expect(sessionStorage.getItem('orch.exports.watched')).toBeNull();
    act(() => { cache.setQueryData(jobQuery(job.id).queryKey, cancelled); });
    expect(screen.getAllByText(i18n.t('export.job.cancelled'), { exact: true })).toHaveLength(1);
  } finally {
    for (const button of screen.queryAllByRole('button', { name: i18n.t('common.dismiss') })) fireEvent.click(button);
    view.unmount(); cache.clear();
  }
});

it.each(['en', 'zh-CN'] as const)('announces the actual partial result before offering its download in %s', async (locale) => {
  await setLocale(locale);
  watchExport(job, 'Team work');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  cache.setQueryData(jobQuery(job.id).queryKey, { ...job, state: 'succeeded', result: { export_id: 'archive', omitted_items: 2 } });
  const view = show(<QueryClientProvider client={cache}><Watch /></QueryClientProvider>);
  try {
    expect(await screen.findByText(locale === 'en' ? 'Team work: partial export · report included' : 'Team work：部分导出 · 内含报告')).toBeVisible();
    expect(screen.getByRole('button', { name: locale === 'en' ? 'Download partial' : '下载部分导出' })).toBeVisible();
    expect(useWatched.getState().items).toMatchObject([{ state: 'ready', exportId: 'archive', omitted: 2 }]);
  } finally {
    for (const button of screen.queryAllByRole('button', { name: i18n.t('common.dismiss') })) fireEvent.click(button);
    view.unmount(); cache.clear();
  }
});

it('keeps an unavailable omission count unknown instead of reporting a complete archive', async () => {
  watchExport(job, 'Team work');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  cache.setQueryData(jobQuery(job.id).queryKey, { ...job, state: 'succeeded', result: { export_id: 'archive', omitted_items: null } });
  const view = show(<QueryClientProvider client={cache}><Watch /></QueryClientProvider>);
  try {
    expect(await screen.findByText('Team work: ready · omission count unavailable')).toBeVisible();
    expect(useWatched.getState().items).toMatchObject([{ state: 'ready', exportId: 'archive', omitted: null }]);
  } finally {
    for (const button of screen.queryAllByRole('button', { name: i18n.t('common.dismiss') })) fireEvent.click(button);
    view.unmount(); cache.clear();
  }
});
