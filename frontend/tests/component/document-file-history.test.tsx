/** File-only work belongs in History under its actual thread, without invented block differences. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import { HistoryDrawer } from '@/features/documents/History';
import type { Author, Document, DocumentChanges, DocumentVersion } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({
  versions: vi.fn<(id: string, author: string | null) => Promise<{ items: DocumentVersion[]; next_cursor: string | null }>>(),
  changes: vi.fn<(id: string, from: string, to: number | null) => Promise<DocumentChanges>>(),
}));
vi.mock('@/api/documents', async (load) => {
  const real = await load<typeof Documents>();
  return { ...real,
    versionsQuery: (id: string, author: string | null) => ({ ...real.versionsQuery(id, author), queryFn: () => calls.versions(id, author) }),
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
    expect(calls.versions).toHaveBeenCalledExactlyOnceWith(doc.id, null);
    await user.keyboard('{Escape}');
    expect(close).toHaveBeenCalledWith(false);
  },
);
