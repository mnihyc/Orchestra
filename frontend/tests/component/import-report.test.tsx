/** Retained import reports page, filter and download their actual object outcomes. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { MouseEventHandler, ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { ImportReport } from '@/features/people/ImportReport';
import type { Import, ImportReportListQuery, ImportReportRow } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ record: vi.fn(), report: vi.fn(), download: vi.fn(), save: vi.fn(), write: vi.fn(), abort: vi.fn(), pick: vi.fn() }));
vi.mock('@/api/imports', async (original) => ({ ...await original<object>(),
  importQuery: (id: string) => ({ queryKey: ['report-record', id], queryFn: () => calls.record(id) as Promise<unknown>, retry: false }),
  importReportQuery: (id: string, filter: ImportReportListQuery) => ({ queryKey: ['report-rows', id, filter], initialPageParam: null,
    queryFn: ({ pageParam }: { pageParam: string | null }) => calls.report(id, filter, pageParam) as Promise<unknown>, getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor, retry: false }),
  downloadImportReport: calls.download,
}));
vi.mock('@/features/people/importReportFile', () => ({ canSaveImportReport: () => true, chooseImportReportFile: calls.pick }));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(), PlaceLink: ({ children, onClick }: { children: ReactNode; onClick?: MouseEventHandler<HTMLAnchorElement> }) => <a href="/project/place" onClick={(event) => { onClick?.(event); event.preventDefault(); }}>{children}</a> }));

const record: Import = {
  id: 'import', node: { id: 'place', kind: 'project', name: 'Web', redacted: false },
  request: { client_id: 'act', source: { files: [{ name: 'notes.zip', blob: 'a'.repeat(64) }] }, destination: {}, options: {}, choices: {} },
  state: 'done', job_id: 'job', undo_job_id: null, files: ['notes.zip'], package: null,
  counts: null, actions: { create: 1, replace: 0, keep_both: 0, skip: 0 }, losses: [], failure: null,
  created_at: '2026-10-28T12:00:00Z', finished_at: '2026-10-28T12:01:00Z', undone_at: null,
};
const row: ImportReportRow = { position: 0, source_id: 'source', kind: 'document', name: 'Plan', path: ['Notes'], action: 'create', outcome: 'created', made: null, message: null, warnings: [], author: null };

beforeEach(() => {
  calls.record.mockReset(); calls.report.mockReset(); calls.download.mockReset(); calls.save.mockReset();
  calls.write.mockReset(); calls.abort.mockReset(); calls.pick.mockReset();
  calls.write.mockResolvedValue(undefined); calls.abort.mockResolvedValue(undefined); calls.save.mockResolvedValue(undefined);
  calls.pick.mockResolvedValue({ write: calls.write, close: calls.save, abort: calls.abort });
  calls.record.mockResolvedValue(record);
  calls.report.mockImplementation((_id: string, filter: ImportReportListQuery, cursor: string | null) => Promise.resolve(filter.outcome === 'failed'
    ? { items: [{ ...row, position: 2, name: 'Failed page', outcome: 'failed', message: 'Could not import' }], next_cursor: null }
    : cursor === null ? { items: [row], next_cursor: 'next' } : { items: [{ ...row, position: 1, name: 'Second page' }], next_cursor: null }));
});
afterEach(async () => { await setLocale('en'); });

it('shows and pages outcomes and asks the server for a failed-only filter', async () => {
  const user = userEvent.setup();
  show(<ImportReport id="import" />);
  expect(await screen.findByText('Plan')).toBeVisible();
  expect(screen.getByText('Done · 1 created')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  expect(await screen.findByText('Second page')).toBeVisible();
  expect(screen.queryByText('Plan')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Back to first page' }));
  expect(await screen.findByText('Plan')).toBeVisible();
  await user.click(screen.getByRole('combobox', { name: 'Results' }));
  await user.click(screen.getByRole('option', { name: 'Failed' }));
  expect(await screen.findByText('Failed page')).toBeVisible();
  expect(screen.getByText('Could not import')).toBeVisible();
  expect(calls.report).toHaveBeenCalledWith('import', { outcome: 'failed' }, null);
});

it('downloads the chosen report format', async () => {
  calls.download.mockResolvedValue({ bytes: 6, name: 'report.csv' });
  const user = userEvent.setup();
  show(<ImportReport id="import" />);
  await screen.findByText('Plan');
  await user.click(screen.getByRole('button', { name: 'Download report' }));
  await user.click(screen.getByRole('menuitem', { name: 'CSV' }));
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith());
  expect(calls.pick).toHaveBeenCalledExactlyOnceWith('csv');
  expect(calls.download).toHaveBeenCalledWith('import', 'csv', expect.any(Function), expect.any(Function));
  expect(calls.abort).not.toHaveBeenCalled();
});

it('does not save a report that finishes after the reader changes', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  try {
    let finish: (file: { bytes: number; name: string }) => void = () => { throw new Error('Download was not started'); };
    calls.download.mockImplementation(() => new Promise<{ bytes: number; name: string }>((resolve) => { finish = resolve; }));
    const user = userEvent.setup();
    show(<ImportReport id="import" />);
    await screen.findByText('Plan');
    await user.click(screen.getByRole('button', { name: 'Download report' }));
    await user.click(screen.getByRole('menuitem', { name: 'JSON' }));
    await waitFor(() => expect(calls.download).toHaveBeenCalledOnce());
    act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
    await act(async () => { finish({ bytes: 7, name: 'report.json' }); await Promise.resolve(); });
    expect(calls.save).not.toHaveBeenCalled();
    expect(calls.abort).toHaveBeenCalledExactlyOnceWith();
  } finally { client.session.setState(before); }
});

it('uses the report reason code in the reader’s language', async () => {
  await setLocale('zh-CN');
  calls.report.mockResolvedValue({ items: [{ ...row, outcome: 'failed', code: 'import.damaged', message: 'The record is damaged' }], next_cursor: null });
  show(<ImportReport id="import" />);
  expect(await screen.findByText('此项与包中的记录不符')).toBeVisible();
  expect(screen.queryByText('The record is damaged')).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('shows the retained failure cause even when no object rows were written in %s', async (locale) => {
  await setLocale(locale);
  const message = 'A unique value is taken (nodes_pkey)';
  calls.record.mockResolvedValue({ ...record, state: 'failed', actions: null, failure: { code: 'internal', message } });
  calls.report.mockResolvedValue({ items: [], next_cursor: null });
  show(<ImportReport id="import" />);
  expect(await screen.findByRole('alert')).toHaveTextContent(message);
});

it('localizes a known retained import failure', async () => {
  await setLocale('zh-CN');
  calls.record.mockResolvedValue({ ...record, state: 'failed', failure: { code: 'import.damaged', message: 'The record is damaged' } });
  show(<ImportReport id="import" />);
  expect(await screen.findByRole('alert')).toHaveTextContent('此项与包中的记录不符');
  expect(screen.queryByText('The record is damaged')).not.toBeInTheDocument();
});

it('offers the shared reference card even when the report has no cached place', async () => {
  calls.report.mockResolvedValue({ items: [{ ...row, made: { id: 'document', kind: 'document', node: null } }], next_cursor: null });
  show(<ImportReport id="import" />);
  expect(await screen.findByRole('link', { name: 'Plan' })).toBeVisible();
});

it('keeps the measured count on a localized history warning', async () => {
  calls.report.mockResolvedValue({ items: [{ ...row, warnings: [{ code: 'history_dropped', message: 'English fallback', params: { count: 2 } }] }], next_cursor: null });
  show(<ImportReport id="import" />);
  expect(await screen.findByText('Some conversation items did not come back · 2')).toBeVisible();
});

it.each([
  ['labels', 'Originally by Original author'],
  [undefined, 'Source author: Original author'],
] as const)('does not infer an account attribution from an absent option', async (authors, expected) => {
  calls.record.mockResolvedValue({ ...record, request: { ...record.request, options: authors === undefined ? {} : { authors } } });
  calls.report.mockResolvedValue({ items: [{ ...row, author: 'Original author' }], next_cursor: null });
  show(<ImportReport id="import" />);
  expect(await screen.findByText(expected)).toBeVisible();
});

it('does not label an account-attributed result as written by the importer', async () => {
  calls.record.mockResolvedValue({ ...record, request: { ...record.request, options: { authors: 'accounts' } } });
  calls.report.mockResolvedValue({ items: [{ ...row, author: 'Original author' }], next_cursor: null });
  show(<ImportReport id="import" />);
  await screen.findByText('Plan');
  expect(screen.queryByText(/Original author/)).not.toBeInTheDocument();
});

it('lets opening the destination close the report dialog', async () => {
  const onOpen = vi.fn(), user = userEvent.setup();
  show(<ImportReport id="import" onOpen={onOpen} />);
  await user.click(await screen.findByRole('link', { name: 'Open Web' }));
  expect(onOpen).toHaveBeenCalledOnce();
});

it.each(['en', 'zh-CN'] as const)('keeps an undone report readable without offering its removed place in %s', async (locale) => {
  await setLocale(locale);
  calls.record.mockResolvedValue({ ...record, state: 'undone', undone_at: '2026-10-28T12:02:00Z' });
  show(<ImportReport id="import" />);
  expect(await screen.findByText('Plan')).toBeVisible();
  expect(screen.queryByRole('link', { name: /Web/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: locale === 'en' ? 'Undo import' : '撤销导入' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: locale === 'en' ? 'Download report' : '下载报告' })).toBeEnabled();
});

it('restores the report filter on this device for the same person', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'person', status: 'signed_in' });
  try {
    const user = userEvent.setup();
    const first = show(<ImportReport id="import" />);
    await screen.findByText('Plan');
    await user.click(screen.getByRole('combobox', { name: 'Results' }));
    await user.click(screen.getByRole('option', { name: 'Failed' }));
    await screen.findByText('Failed page');
    first.unmount();
    show(<ImportReport id="import" />);
    expect(await screen.findByText('Failed page')).toBeVisible();
    expect(screen.getByRole('combobox', { name: 'Results' })).toHaveTextContent('Failed');
  } finally { client.session.setState(before); }
});
