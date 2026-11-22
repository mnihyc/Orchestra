/** Live filename search preserves its act and continuation without a complete file index. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import { NO_SEARCH, SearchBox, SearchResults } from '@/features/files/SearchInFiles';
import { show } from './support';
import ArchiveViewer from '@/features/files/viewers/Archive';
import type { FileHead } from '@/features/files/content';
import { ApiError } from '@/api/client';
import { currentLocale, i18n, setLocale } from '@/i18n';

const calls = vi.hoisted(() => ({ names: vi.fn<typeof Workspace.searchNames>(), content: vi.fn<typeof Workspace.searchContent>(), archive: vi.fn<typeof Workspace.archivePage>(), entry: vi.fn<typeof Workspace.readArchiveEntry>() }));
vi.mock('@/api/workspace', async (original) => ({ ...await original<typeof Workspace>(), searchNames: calls.names, searchContent: calls.content, archivePage: calls.archive, readArchiveEntry: calls.entry }));
vi.mock('@/ui/CodeView', async (original) => ({ ...await original<object>(), CodeView: ({ text }: { text: string }) => <pre>{text}</pre> }));
beforeEach(() => { calls.entry.mockReset(); calls.names.mockReset(); calls.content.mockReset(); calls.archive.mockReset(); });
afterEach(cleanup);

it('keeps the Names choice available without requiring a complete index', () => {
  show(<SearchBox state={NO_SEARCH} onChange={vi.fn()} directory="" />);
  expect(screen.getByRole('radio', { name: 'Names' })).not.toBeDisabled();
});

it('continues a live traversal even when its first time slice found no matching name', async () => {
  const source = { source: 'live' as const, as_of: '2026-11-14T00:00:00Z', observed_from: '2026-11-14T00:00:00Z' };
  calls.names.mockResolvedValueOnce({ ...source, entries: [], next_cursor: 'next', searched: 100 });
  calls.names.mockResolvedValueOnce({ ...source, entries: [{ name: 'needle', path: 'deep/needle', type: 'dir', size: 0, mtime: source.as_of, mode: 0o755, link_target: null, cached: false }], next_cursor: null, searched: 200 });
  show(<SearchResults door={{ environmentId: 'environment', threadId: null }} state={{ ...NO_SEARCH, text: 'needle' }} directory="" onOpenPath={vi.fn()} onOpenFolder={vi.fn()} />);
  const more = await screen.findByRole('button', { name: 'Continue search' });
  await userEvent.click(more);
  await waitFor(() => expect(screen.getByRole('button', { name: 'deep/needle' })).toBeInTheDocument());
  expect(calls.names.mock.calls[1]?.[3]).toBe('next');
});


it('keeps earlier archive entries visible while continuing the same listing', async () => {
  const head: FileHead = { stat: { path: 'packed.zip', name: 'packed.zip', type: 'file', size: 200, mtime: '2026-11-14T00:00:00Z', mode: 0o644, cached: false, link_target: null, hash: null, media_type: 'application/zip', is_text: false }, brought: { state: 'ready', hash: 'a'.repeat(64), size: 200 } };
  const about = { format: 'zip' as const, compression: null, encrypted: false };
  calls.archive.mockResolvedValueOnce({ ...about, entries: [{ path: 'first.txt', kind: 'file', size: null, modified_at: null }], truncated: true, next_cursor: 'archive-next' });
  calls.archive.mockResolvedValueOnce({ ...about, entries: [{ path: 'last.txt', kind: 'file', size: null, modified_at: null }], truncated: false, next_cursor: null });
  show(<ArchiveViewer door={{ environmentId: 'environment', threadId: null }} path="packed.zip" head={head} line={null} tabId="archive" whyReadOnly={null} thread={null} openAs={vi.fn()} />);
  expect(await screen.findByRole('button', { name: 'first.txt' })).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Continue listing' }));
  expect(await screen.findByRole('button', { name: 'last.txt' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'first.txt' })).toBeInTheDocument();
  expect(calls.archive.mock.calls[1]?.[3]).toMatchObject({ cursor: 'archive-next' });
});

for (const locale of ['en', 'zh-CN'] as const) for (const expired of [false, true]) {
  it.each(['names', 'content'] as const)(`retains %s rows after a failed continuation in ${locale}, expired=${expired}`, async (mode) => {
    const previous = currentLocale(); await setLocale(locale);
    try {
      const source = { source: 'live' as const, as_of: '2026-11-14T00:00:00Z', observed_from: '2026-11-14T00:00:00Z', searched: 1 };
      const failure = new ApiError({ type: 'orch:error:' + (expired ? 'conflict.stale_revision' : 'environment.unavailable'),
        code: expired ? 'conflict.stale_revision' : 'environment.unavailable', title: '', detail: '', status: expired ? 409 : 503, request_id: '' }, null);
      const hit = { ...source, matches: [{ path: 'first.txt', line: 1, text: 'needle retained', ranges: [[0, 6] as [number, number]] }], truncated: true, next_cursor: 'search-next' };
      if (mode === 'names') calls.names.mockResolvedValueOnce({ ...source, entries: [{ name: 'needle', path: 'deep/needle', type: 'dir', size: 0, mtime: source.as_of, mode: 0o755, link_target: null, cached: false }], next_cursor: 'search-next' }).mockRejectedValueOnce(failure);
      else calls.content.mockResolvedValueOnce(hit).mockRejectedValueOnce(failure);
      show(<SearchResults door={{ environmentId: 'environment', threadId: null }} state={{ ...NO_SEARCH, text: 'needle', mode }} directory="" onOpenPath={vi.fn()} onOpenFolder={vi.fn()} />);
      await userEvent.click(await screen.findByRole('button', { name: i18n.t('files.search.more') }));
      expect(await screen.findByRole('button', { name: i18n.t(expired ? 'files.search.restart' : 'files.search.retry') })).toBeVisible();
      expect(screen.getByTestId('search-results')).toHaveTextContent(mode === 'names' ? 'deep/needle' : 'needle retained');
    } finally { cleanup(); await setLocale(previous); }
  });
  it(`retains archive rows and selection after a failed continuation in ${locale}, expired=${expired}`, async () => {
    const previous = currentLocale(); await setLocale(locale);
    try {
      const failure = new ApiError({ type: 'orch:error:' + (expired ? 'conflict.stale_revision' : 'environment.unavailable'),
        code: expired ? 'conflict.stale_revision' : 'environment.unavailable', title: '', detail: '', status: expired ? 409 : 503, request_id: '' }, null);
      calls.archive.mockResolvedValueOnce({ format: 'zip', compression: null, encrypted: false,
        entries: [{ path: 'first.txt', kind: 'file', size: null, modified_at: null }], truncated: true, next_cursor: 'archive-next' }).mockRejectedValueOnce(failure);
      calls.entry.mockResolvedValue(new TextEncoder().encode('selected content'));
      const head: FileHead = { stat: { path: 'packed.zip', name: 'packed.zip', type: 'file', size: 200, mtime: '2026-11-14T00:00:00Z', mode: 0o644, cached: false, link_target: null, hash: null, media_type: 'application/zip', is_text: false }, brought: { state: 'ready', hash: 'a'.repeat(64), size: 200 } };
      show(<ArchiveViewer door={{ environmentId: 'environment', threadId: null }} path="packed.zip" head={head} line={null} tabId="archive" whyReadOnly={null} thread={null} openAs={vi.fn()} />);
      await userEvent.click(await screen.findByRole('button', { name: 'first.txt' }));
      await userEvent.click(screen.getByRole('button', { name: i18n.t('files.archive.more') }));
      expect(await screen.findByRole('button', { name: i18n.t(expired ? 'files.archive.restart' : 'files.archive.retry') })).toBeVisible();
      expect(screen.getByRole('treeitem')).toHaveAttribute('aria-selected', 'true');
      expect(screen.getByRole('button', { name: 'first.txt' })).toBeVisible();
      expect(screen.getByTestId('archive-entry')).toHaveTextContent('selected content');
    } finally { cleanup(); await setLocale(previous); }
  });
}
