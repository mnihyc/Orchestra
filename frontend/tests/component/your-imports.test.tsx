/** The personal import history opens reports and keeps all source names available. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as Imports from '@/api/imports';
import { YourImports } from '@/features/people/YourImports';
import type { ImportLine } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ list: vi.fn(), files: vi.fn(), open: vi.fn(), report: vi.fn(), job: vi.fn() }));
vi.mock('@/api/imports', async (original) => {
  const actual = await original<typeof Imports>();
  return { ...actual, myImportsQuery: () => ({ queryKey: actual.importKeys.mine(), initialPageParam: null,
    queryFn: () => calls.list() as Promise<unknown>, getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor, retry: false }),
    importFileNamesQuery: (id: string) => ({ queryKey: ['import-files', id], initialPageParam: null,
      queryFn: () => calls.files(id) as Promise<unknown>, getNextPageParam: (page: { next_cursor: string | null }) => page.next_cursor, retry: false }) };
});
vi.mock('@/features/people/importHostState', () => ({ openImport: calls.open, openImportReport: calls.report, openImportJob: calls.job }));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <section>{children}</section> }));
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(), PlaceLink: ({ children }: { children: ReactNode }) => <span>{children}</span> }));

const row: ImportLine = {
  id: 'import', node: { id: 'place', kind: 'project', name: 'Web', redacted: false }, state: 'done', job_id: 'job', undo_job_id: null,
  first_file: 'Notes/one.md', file_count: 2, actions: { create: 1, keep_both: 0, replace: 0, skip: 0 },
  created_at: '2026-10-28T12:00:00Z', finished_at: '2026-10-28T12:01:00Z', undone_at: null,
};

let previousSession = client.session.getState();
beforeEach(() => {
  previousSession = client.session.getState();
  client.session.setState({ ...previousSession, userId: 'first', readerVersion: 1, status: 'signed_in' });
  calls.list.mockReset(); calls.files.mockReset(); calls.open.mockReset(); calls.report.mockReset(); calls.job.mockReset();
});
afterEach(() => { client.session.setState(previousSession); });

it('offers a new import from an empty history', async () => {
  calls.list.mockResolvedValue({ items: [], next_cursor: null });
  const user = userEvent.setup();
  show(<YourImports />);
  expect(await screen.findByText('No imports yet')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Import…' }));
  expect(calls.open).toHaveBeenCalledOnce();
});

it('opens a retained report and shows the remaining source names', async () => {
  calls.list.mockResolvedValue({ items: [row], next_cursor: null });
  calls.files.mockResolvedValue({ items: ['Notes/one.md', 'Notes/two.md'], next_cursor: null });
  const user = userEvent.setup();
  show(<YourImports />);
  expect(await screen.findByText('Web')).toBeVisible();
  await user.click(screen.getByRole('button', { name: '+1' }));
  expect(await screen.findByText('Notes/two.md')).toBeVisible();
  expect(calls.files).toHaveBeenCalledExactlyOnceWith('import');
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button', { name: 'Report for Web' }));
  expect(calls.report).toHaveBeenCalledExactlyOnceWith('import');
});

it('never displays the prior reader’s cached history after an account change', async () => {
  calls.list.mockResolvedValueOnce({ items: [row], next_cursor: null }).mockImplementationOnce(() => new Promise(() => undefined));
  show(<YourImports />);
  expect(await screen.findByText('Web')).toBeVisible();
  act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
  expect(screen.queryByText('Web')).not.toBeInTheDocument();
  expect(screen.queryByText('Notes/one.md')).not.toBeInTheDocument();
  await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(2));
});
