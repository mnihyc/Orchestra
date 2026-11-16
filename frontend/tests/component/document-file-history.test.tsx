/** File-only work belongs in History under its actual thread, without invented block differences. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import { HistoryDrawer } from '@/features/documents/History';
import type { Author, Document, DocumentChanges, DocumentVersion } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { World } from '../doubles/world';
import { deferred } from './deferred';
import { show } from './support';

interface VersionsPage { items: DocumentVersion[]; next_cursor: string | null }

const calls = vi.hoisted(() => ({
  versions: vi.fn<(id: string, author: string | null, cursor: string | null) => Promise<VersionsPage>>(),
  changes: vi.fn<(id: string, from: string, to: number | null) => Promise<DocumentChanges>>(),
}));
vi.mock('@/api/documents', async (load) => {
  const real = await load<typeof Documents>();
  return { ...real,
    versionsQuery: (id: string, author: string | null) => ({ ...real.versionsQuery(id, author), queryFn: ({ pageParam }: { pageParam: string | null }) => calls.versions(id, author, pageParam) }),
    changesQuery: (id: string, from: string, to: number | null) => ({ ...real.changesQuery(id, from, to), queryFn: () => calls.changes(id, from, to) }),
  };
});
const before = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
const oldDark = document.documentElement.classList.contains('dark');
let doc: Document;
const writer: Author = { agent: 'Build agent', thread: { id: 'writer-thread', kind: 'thread', name: 'Prepare results', redacted: false } };

beforeEach(() => {
  calls.versions.mockReset(); calls.changes.mockReset();
  const world = new World(), ada = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((one) => one.owner === 'ada' && !one.deleted);
  if (ada === undefined || stored === undefined) throw new Error('History needs its ordinary readable document fixture');
  doc = { ...world.documents.full(ada, stored), version: 1 };
  const entry: DocumentVersion = { version: null, title: doc.title, bytes: 0, block_count: 0, cause: null,
    authors: [writer], changed: { inserted: 0, updated: 0, moved: 0, removed: 0, files: [
      { name: 'result.bin', kind: 'added' }, { name: 'report.csv', kind: 'replaced' }, { name: 'old.png', kind: 'removed' },
    ] }, from: '2026-10-16T02:00:00Z', to: '2026-10-16T02:01:00Z' };
  calls.versions.mockResolvedValue({ items: [entry], next_cursor: null });
  calls.changes.mockResolvedValue({ from: 1, to: null, title: null, blocks: [], authors: [writer] });
});
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', oldDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)(
  'reads only an open drawer, names its thread once and shows all file changes in %s/%s', async (locale, theme) => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), close = vi.fn();
    const view = show(<HistoryDrawer doc={doc} open={false} onOpenChange={close} />);
    expect(calls.versions).not.toHaveBeenCalled(); expect(calls.changes).not.toHaveBeenCalled();
    view.rerender(<HistoryDrawer doc={doc} open onOpenChange={close} />);
    const entries = await screen.findByRole('list', { name: i18n.t('documents.history.entries') });
    const entry = within(entries).getByRole('button');
    expect(within(entry).getAllByText('Build agent')).toHaveLength(1);
    for (const key of ['documents.history.filesAdded', 'documents.history.filesReplaced', 'documents.history.filesRemoved'] as const) {
      expect(entry).toHaveTextContent(i18n.t(key, { count: 1 }));
    }
    expect(calls.changes).not.toHaveBeenCalled();
    await user.click(entry);
    const files = await screen.findByRole('list', { name: i18n.t('documents.history.fileDifference') });
    for (const [kind, name] of [['added', 'result.bin'], ['replaced', 'report.csv'], ['removed', 'old.png']] as const) {
      expect(within(files).getAllByText(i18n.t(`documents.history.file_${kind}`, { name }))).toHaveLength(1);
    }
    await waitFor(() => expect(calls.changes).toHaveBeenCalledExactlyOnceWith(doc.id, '1', null));
    const detail = screen.getByRole('region', { name: i18n.t('documents.history.detail') });
    await waitFor(() => expect(within(detail).queryByRole('status')).not.toBeInTheDocument());
    expect(screen.queryByText(i18n.t('documents.history.nothing'))).not.toBeInTheDocument();
    expect(screen.queryByRole('list', { name: i18n.t('documents.history.difference') })).not.toBeInTheDocument();
    expect(screen.queryByRole('radiogroup', { name: i18n.t('documents.history.layout') })).not.toBeInTheDocument();
    expect(calls.versions).toHaveBeenCalledExactlyOnceWith(doc.id, null, null);
    await user.keyboard('{Escape}');
    expect(close).toHaveBeenCalledWith(false);
  },
);

function version(number: number, authors: Author[] = [writer]): DocumentVersion {
  return { version: number, title: `Saved ${number}`, bytes: 4096, block_count: 1, cause: 'window', authors,
    changed: { inserted: 0, updated: 1, moved: 0, removed: 0 },
    from: '2026-10-01T02:00:00Z', to: '2026-10-01T02:01:00Z' };
}

it.each(['en', 'zh-CN'] as const)('counts loaded versions through another page and an author filter without inventing retention in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const other: Author = { agent: 'Review agent', thread: { id: 'review-thread', kind: 'thread', name: 'Review results', redacted: false } };
  const open = { ...version(8), version: null, cause: null };
  calls.versions.mockImplementation((_id, author, cursor) => Promise.resolve(author === null
    ? cursor === null ? { items: [open, version(7), version(6, [other])], next_cursor: 'older' }
      : { items: [version(5, [other])], next_cursor: null }
    : { items: [open, version(7)], next_cursor: null }));
  show(<HistoryDrawer doc={doc} open onOpenChange={vi.fn()} />);
  const loaded = (count: number) => locale === 'en' ? `Loaded ${count} ${count === 1 ? 'version' : 'versions'}` : `已加载 ${count} 个版本`;
  const first = await screen.findByText(loaded(2));
  expect(first).not.toHaveTextContent(/Kept|back to|已保留|最早到/);
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  await screen.findByText(loaded(3));
  expect(calls.versions).toHaveBeenLastCalledWith(doc.id, null, 'older');
  await user.click(screen.getByRole('combobox', { name: i18n.t('documents.history.author') }));
  await user.click(screen.getByRole('option', { name: 'Build agent' }));
  await screen.findByText(loaded(1));
  expect(calls.versions).toHaveBeenLastCalledWith(doc.id, 'thread:writer-thread', null);
  expect(screen.getByRole('list', { name: i18n.t('documents.history.entries') }).querySelectorAll('[data-version]')).toHaveLength(2);
});

it.each(['en', 'zh-CN'] as const)('keeps an empty author result separate from global retention in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  calls.versions.mockImplementation((_id, author) => Promise.resolve({ items: author === null ? [version(7)] : [], next_cursor: null }));
  show(<HistoryDrawer doc={doc} open onOpenChange={vi.fn()} />);
  await screen.findByRole('list', { name: i18n.t('documents.history.entries') });
  await user.click(screen.getByRole('combobox', { name: i18n.t('documents.history.author') }));
  await user.click(screen.getByRole('option', { name: 'Build agent' }));
  expect(await screen.findByText(locale === 'en' ? 'Loaded 0 versions' : '已加载 0 个版本')).toBeVisible();
  expect(screen.getByText(locale === 'en' ? 'No history entries' : '没有历史记录')).toBeVisible();
  expect(screen.queryByText(/No version has been kept|尚未保留任何版本/)).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('makes no version count or retention claim before a read succeeds in %s', async (locale) => {
  await setLocale(locale);
  const pending = deferred<VersionsPage>();
  calls.versions.mockReturnValue(pending.promise);
  show(<HistoryDrawer doc={doc} open onOpenChange={vi.fn()} />);
  const claim = /Loaded \d+ versions|已加载.*个版本|No version has been kept|尚未保留任何版本/;
  expect(screen.queryByText(claim)).not.toBeInTheDocument();
  await act(async () => { pending.reject(new Error('History read failed')); await pending.promise.catch(() => undefined); });
  await screen.findByRole('alert');
  expect(screen.queryByText(claim)).not.toBeInTheDocument();
});
