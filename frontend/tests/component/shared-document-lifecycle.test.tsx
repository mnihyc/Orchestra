/** Shared is a usable entrance for a document even when no hierarchy parent is readable. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import { SharedScreen } from '@/features/shared/Shared';
import type { DocumentRef, SharedListReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { id, MADE_AT } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<(cursor: string | null) => Promise<SharedListReply>>() }));
vi.mock('@/api/shared', () => ({ sharedPageQuery: (cursor: string | null) => ({
  queryKey: ['shared-document-lifecycle', cursor], queryFn: () => calls.read(cursor),
}) }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  Link: ({ to, params, children, className }: { to: string; params?: Record<string, string>; children?: ReactNode; className?: string }) =>
    <a href={Object.entries(params ?? {}).reduce((path, [key, value]) => path.replace(`$${key}`, encodeURIComponent(value)), to)} className={className}>{children}</a>,
}));
const document: DocumentRef = {
  id: id(98, 1), title: 'Shared research', description: '', description_source: 'auto', description_line: '',
  node: { id: id(98, 2), kind: 'topic', name: null, redacted: true },
  owner: ADA, owners: 1, created_by: ADA, page: 'restricted', seq: 0, block_count: 0, bytes: 0, files_bytes: 0,
  version: 0, open_discussions: 0, updated_at: MADE_AT, updated_by: null, discussion_level: 'involved', seen_seq: null,
  archived: false, deleted: false,
};
const before = useAppearance.getState();
beforeEach(() => {
  vi.clearAllMocks();
  calls.read.mockImplementation((cursor) => Promise.resolve(cursor === null
    ? { items: [], partial: true, next_cursor: 'after-hidden' }
    : { items: [{ kind: 'document', document, path: [document.node], participating: true }], partial: false, next_cursor: null }));
});
afterEach(async () => { cleanup(); useAppearance.setState(before); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens a document after an empty scanned page and removes a revoked share on refresh in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    show(<SharedScreen />);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('documentFolders.next') }));
    const link = await screen.findByRole('link', { name: document.title });
    expect(link).toHaveAttribute('href', `/doc/${document.id}`);
    expect(screen.queryByRole('link', { name: 'Never reveal parent' })).not.toBeInTheDocument();
    expect(calls.read.mock.calls.map(([cursor]) => cursor)).toEqual([null, 'after-hidden']);
    expect(screen.getByRole('button', { name: i18n.t('documentFolders.next') })).toBeDisabled();
    calls.read.mockResolvedValue({ items: [], partial: false, next_cursor: null });
    await userEvent.click(screen.getByRole('button', { name: i18n.t('sharedWithMe.refresh') }));
    await waitFor(() => expect(screen.queryByRole('link', { name: document.title })).not.toBeInTheDocument());
    expect(calls.read.mock.calls.map(([cursor]) => cursor)).toEqual([null, 'after-hidden', 'after-hidden']);
  });
}
