/** Background actions remain reachable after an ordinary notice would have expired. */
import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { TOAST_SHOWN_MS, toast } from '@/ui/Toast';
import { show } from './support';

const dismiss: (() => void)[] = [];
afterEach(async () => { act(() => { for (const close of dismiss.splice(0)) close(); }); cleanup(); vi.useRealTimers(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['export', 'import'] as const)('keeps a completed '+locale+' %s action until it is used', async (kind) => {
    await setLocale(locale); vi.useFakeTimers();
    show(<span />);
    const run = vi.fn();
    const label = i18n.t(kind === 'export' ? 'export.downloadButton' : 'importWizard.job.viewReport');
    const title = i18n.t(kind === 'export' ? 'export.job.ready' : 'importWizard.job.finished', { place: 'Team work' });
    act(() => { dismiss.push(toast({ tone: 'success', title, action: { label, run } })); });
    await act(async () => { await vi.advanceTimersByTimeAsync(TOAST_SHOWN_MS * 2); });
    expect(screen.getByRole('button', { name: label })).toBeVisible();
    expect(run).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: label }));
    expect(run).toHaveBeenCalledOnce();
    expect(screen.queryByRole('button', { name: label })).not.toBeInTheDocument();
  });

  it('lets a person dismiss an unused action in '+locale, async () => {
    await setLocale(locale); vi.useFakeTimers();
    show(<span />);
    const run = vi.fn(), label = i18n.t('export.downloadButton');
    act(() => { dismiss.push(toast({ tone: 'success', title: i18n.t('export.job.ready', { place: 'Team work' }), action: { label, run } })); });
    fireEvent.click(screen.getByRole('button', { name: i18n.t('common.dismiss') }));
    expect(screen.queryByRole('button', { name: label })).not.toBeInTheDocument();
    expect(run).not.toHaveBeenCalled();
  });
}

it('expires an ordinary success at its named default and keeps a failure', async () => {
  vi.useFakeTimers(); show(<span />);
  act(() => {
    dismiss.push(toast({ tone: 'success', title: 'Saved the preference' }));
    dismiss.push(toast({ tone: 'danger', title: 'The background task failed' }));
  });
  await act(async () => { await vi.advanceTimersByTimeAsync(TOAST_SHOWN_MS * 2); });
  expect(screen.queryByText('Saved the preference', { exact: true })).not.toBeInTheDocument();
  expect(screen.getByText('The background task failed', { exact: true })).toBeVisible();
});
