/** The document Move door keeps native deletion witnesses, folder destinations and one deliberate write. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Destinations from '@/api/copyDestinations';
import type * as Moves from '@/api/documentMove';
import type * as Tree from '@/api/documentTree';
import type * as Library from '@/api/resourceLibrary';
import type * as Nodes from '@/api/nodes';
import type * as Search from '@/api/search';
import type * as Router from '@tanstack/react-router';
import { ApiError } from '@/api/client';
import { TreeActionDialog } from '@/features/documents/TreeActions';
import type { DocumentDirectory, DocumentDirectoryContext, DocumentTreeListed, NameHit } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { org, me, userRef } from '../doubles/fixtures';
import { treeNode } from '../doubles/tree-node';
import { show } from './support';
const calls = vi.hoisted(() => ({ preview: vi.fn<typeof Moves.previewDocumentMoveDeletion>(), execute: vi.fn<typeof Moves.executeDocumentMove>(), same: vi.fn<() => boolean>(), navigate: vi.fn<(options: { href: string }) => Promise<void>>(), cross: true }));
vi.mock('@/api/documentMove', async (original) => ({ ...await original<typeof Moves>(), previewDocumentMoveDeletion: calls.preview, executeDocumentMove: calls.execute }));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(), sameLibraryReader: () => calls.same() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/copyDestinations', async (original) => ({ ...await original<typeof Destinations>(), copyDestinationsQuery: (request: Destinations.CopyDestinationsRequest) => ({ queryKey: ['document-move-destinations', request], queryFn: () => Promise.resolve({ subject: request.subject, intent: request.intent, source_owned: true, destinations: request.destinations.map((destination) => ({ destination, cross_root: calls.cross, outcome: { state: 'needs_choice', choice: calls.cross ? 'delete_preview' : 'in_org_move' } })) }) }) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), orgsQuery: { queryKey: ['document-move-orgs'], queryFn: () => Promise.resolve([org('other-org', 'other', 'Other org', userRef(me(1, 'ada', 'Ada')))]) }, fetchTree: () => Promise.resolve({ seq: 1, since: null, complete: true, hidden: [], nodes: [treeNode({ id: 'other-org', kind: 'org', parent_id: null, name: 'Other org' })] }) }));
vi.mock('@/api/documentTree', async (original) => ({ ...await original<typeof Tree>(),
  documentDirectoryQuery: (request: Tree.DocumentDirectoryRequest) => ({ queryKey: ['document-move-directory', request.at], queryFn: () => Promise.resolve({ directory: context(request.at), items: [], ancestors: [], feed: 'directory', next_cursor: null }) }),
  documentTreeEntryQuery: (item: { kind: string; id: string }) => ({ queryKey: ['document-move-folder', item], queryFn: () => Promise.resolve({ entries: [], ancestors: [], directory: context({ kind: 'container', id: item.id }) }) }),
}));
const folderHit: NameHit = { id: 'target-folder', kind: 'document_container', name: 'Notes', home_path: [], path: [], address: null, avatar: null, archived: false, color: null, custom_color: null, owner: null, recent: true, tag: null, thread_attention: null, username: null };
vi.mock('@/api/search', async (original) => ({ ...await original<typeof Search>(), namesQuery: () => ({ queryKey: ['document-move-folder-names'], queryFn: () => Promise.resolve([folderHit]) }) }));
const source = (kind: 'document' | 'container' = 'document'): DocumentTreeListed => ({ entry: { item: { kind, id: 'source' }, parent: { kind: 'node_docs', id: 'source-node' }, node: 'source-node', root: 'source-org', basename: kind === 'document' ? 'Report.md' : 'Research', title: 'Report', owner: 'author', revision: 7, files: 2, versions: 3, deleted_at: null, deletion: null }, standing: { read: true, write: true, manage: true } });
function context(at: DocumentDirectory): DocumentDirectoryContext {
  return { at, node: { kind: 'org', id: 'other-org', name: 'Other org', redacted: false }, path: [{ kind: 'org', id: 'other-org', name: 'Other org', redacted: false }], folder: at.kind === 'node_docs' ? null : { ...source('container'), entry: { ...source('container').entry, item: { kind: 'container', id: at.id }, root: 'other-org', node: 'other-org', parent: { kind: 'node_docs', id: 'other-org' }, title: 'Notes', basename: 'Notes' } }, ancestors: [], can_create: true, inspection: null, membership_seq: 1 };
}
const full = () => ({ stamp: { entries: [source().entry], authority_epoch: 1, authority_fingerprint: 'native-membership', authority_until: null }, gaining: 0, losing: 1, restore_until: '2030-01-01T00:00:00Z' });
beforeEach(() => { vi.clearAllMocks(); calls.cross = true; calls.same.mockReturnValue(true); calls.navigate.mockResolvedValue(undefined); calls.preview.mockResolvedValue(full()); calls.execute.mockResolvedValue({ projection: 'receipt', operation: 'native-move', entries: [{ kind: 'document', id: 'new-doc' }] }); });
afterEach(async () => { cleanup(); await setLocale('en'); });
async function choose(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') })); const option = await screen.findByRole('option', { name: 'Other org' });
  await waitFor(() => expect(option).not.toHaveAttribute('aria-disabled', 'true')); await user.click(option);
}
const confirm = () => screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`reviews actual membership, cancels, then explicitly moves once (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(), close = vi.fn(); let view = show(<TreeActionDialog command="move" rows={[source()]} onClose={close} />); await choose(user);
    await screen.findByRole('table', { name: i18n.t('duplicate.deletePlan') }); expect(calls.execute).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') })); expect(close).toHaveBeenCalledOnce(); expect(calls.execute).not.toHaveBeenCalled();
    view.unmount(); view = show(<TreeActionDialog command="move" rows={[source()]} onClose={close} />); await choose(user); await screen.findByRole('table'); await user.click(confirm());
    await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[0]).toMatchObject({ command: 'move_out', items: [{ kind: 'document', id: 'source' }], to: { kind: 'node_docs', id: 'other-org' }, deletion_plan: full().stamp });
    await waitFor(() => expect(calls.navigate.mock.calls.at(-1)?.[0].href).toMatch(/\/doc\/new-doc$/u)); view.unmount();
  });
  it(`a protected preview reveals no membership counts and remains usable (${locale})`, async () => {
    await setLocale(locale); calls.preview.mockResolvedValue({ projection: 'receipt', deletion_plan: { confirmation: 'native-sealed' } }); const user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[source()]} onClose={() => undefined} />); await choose(user);
    await screen.findByText(i18n.t('duplicate.deletePlanReady')); expect(screen.queryByRole('table')).not.toBeInTheDocument(); await user.click(confirm());
    await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[0].deletion_plan).toEqual({ confirmation: 'native-sealed' });
  });
  it(`the same-root Move uses its identity-preserving command without a deletion preview (${locale})`, async () => {
    await setLocale(locale); calls.cross = false; const row = source(); row.entry.root = 'other-org'; calls.execute.mockResolvedValue({ projection: 'receipt', operation: 'move', entries: [row.entry.item] }); const user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[row]} onClose={() => undefined} />); await choose(user);
    const button = screen.getByRole('button', { name: i18n.t('duplicate.moveConfirm') }); await waitFor(() => expect(button).toBeEnabled()); await user.click(button);
    await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[0]).toMatchObject({ command: 'move', deletion_plan: null }); expect(calls.preview).not.toHaveBeenCalled(); expect(calls.navigate).not.toHaveBeenCalled();
  });
  it(`a failed preview keeps the selected org and sends no operation (${locale})`, async () => {
    await setLocale(locale); calls.preview.mockRejectedValueOnce(new NetworkError('failed')); const user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[source()]} onClose={() => undefined} />); await choose(user); await screen.findByRole('alert');
    expect(confirm()).toBeDisabled(); expect(screen.getByText('Other org', { exact: true })).toBeInTheDocument(); expect(calls.execute).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') })); await screen.findByRole('table'); expect(calls.execute).not.toHaveBeenCalled();
  });
}
it('a failed answer retries the exact client identity and original complete witness', async () => {
  calls.execute.mockRejectedValueOnce(new NetworkError('failed')); const user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[source()]} onClose={() => undefined} />); await choose(user); await screen.findByRole('table'); await user.click(confirm()); await screen.findByRole('alert'); await user.click(confirm());
  await waitFor(() => expect(calls.execute).toHaveBeenCalledTimes(2)); expect(calls.execute.mock.calls[1]).toEqual(calls.execute.mock.calls[0]);
});
it('a stale membership requires a deliberate fresh preview and confirmation', async () => {
  calls.execute.mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Stale', detail: '', request_id: 'stale-tree' }, null)); const user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[source()]} onClose={() => undefined} />); await choose(user); await screen.findByRole('table'); await user.click(confirm()); await screen.findByRole('alert');
  calls.preview.mockResolvedValue({ projection: 'receipt', deletion_plan: { confirmation: 'current-after-stale' } }); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.reviewAgain') })); await screen.findByText(i18n.t('duplicate.deletePlanReady')); expect(calls.execute).toHaveBeenCalledOnce();
  await user.click(confirm()); await waitFor(() => expect(calls.execute).toHaveBeenCalledTimes(2)); expect(calls.execute.mock.calls[1]?.[0].deletion_plan).toEqual({ confirmation: 'current-after-stale' }); expect(calls.execute.mock.calls[1]?.[0].client_id).not.toBe(calls.execute.mock.calls[0]?.[0].client_id);
});
it('a real folder choice stays a document-only container destination', async () => {
  const user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[source()]} onClose={() => undefined} />); await choose(user); await screen.findByRole('table');
  await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.folderOptional') })); await user.click(await screen.findByRole('option', { name: 'Notes' })); await waitFor(() => expect(confirm()).toBeEnabled()); await user.click(confirm());
  await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[0].to).toEqual({ kind: 'container', id: 'target-folder' });
});
it('a late accepted move never opens or closes under a different reader', async () => {
  let finish: ((value: Awaited<ReturnType<typeof Moves.executeDocumentMove>>) => void) | undefined; calls.execute.mockImplementation(() => new Promise((resolve) => { finish = resolve; })); const close = vi.fn(), user = userEvent.setup(); show(<TreeActionDialog command="move" rows={[source()]} onClose={close} />); await choose(user); await screen.findByRole('table'); await user.click(confirm()); await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); calls.same.mockReturnValue(false);
  await act(async () => { finish?.({ projection: 'receipt', operation: 'accepted', entries: [{ kind: 'document', id: 'new-doc' }] }); await Promise.resolve(); }); expect(close).not.toHaveBeenCalled(); expect(calls.navigate).not.toHaveBeenCalled();
});
