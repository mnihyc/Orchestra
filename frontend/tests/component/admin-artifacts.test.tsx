/** Stored artifacts expose actual custody facts and safe native readers in both languages. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { OwnUploads, UploadOpen, UploadState, UploadAttachments } from '@/features/uploads/Inventory';
import { EnvironmentSnapshots } from '@/features/resources/environments/Snapshots';
import { ArtifactOpen } from '@/features/storage/Artifacts';
import { i18n, setLocale } from '@/i18n';
import { reachExport, reachImport, reachImportRow, reachPerson, reachSnapshot, reachUpload } from '../doubles/admin-reach';
import { me, RUNS_ON } from '../doubles/fixtures';
import type { HttpAnswer, HttpTransport } from '../doubles/http-wire';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>(), remove: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.get, DELETE: calls.remove } }; });
const answer = (data: unknown): HttpAnswer => ({ data, response: new Response(null, { status: 200 }) });
const caches: QueryClient[] = [];
function cache() {
  const c = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  c.setQueryData(['me'], { ...me(1, 'ada', 'Ada'), is_system_admin: true });
  c.setQueryData(['admin-mode'], { as_admin: true, available: true }); caches.push(c); return c;
}
beforeEach(() => {
  calls.get.mockReset().mockImplementation((path) => Promise.resolve(answer(path === '/exports/{id}' ? reachExport()
    : path === '/imports/{id}' ? reachImport() : path === '/imports/{id}/report' ? { items: [reachImportRow], next_cursor: null }
    : { items: [reachUpload()], next_cursor: null })));
  calls.remove.mockReset().mockResolvedValue({ response: new Response(null, { status: 204 }) });
});
afterEach(async () => { cleanup(); caches.splice(0).forEach((c) => c.clear()); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`personal uploads cancel/release only after the effect is confirmed (${locale})`, async () => {
    await setLocale(locale); show(<QueryClientProvider client={cache()}><OwnUploads /></QueryClientProvider>);
    const table = await screen.findByRole('table', { name: i18n.t('adminArtifacts.uploads') });
    expect(table).toHaveTextContent('orphan.txt');
    await userEvent.click(within(table).getByRole('button', { name: i18n.t('adminArtifacts.delete') }));
    let confirm = screen.getByRole('alertdialog');
    expect(confirm).toHaveTextContent(i18n.t('adminArtifacts.releaseUploadBody'));
    await userEvent.click(within(confirm).getByRole('button', { name: i18n.t('common.cancel') }));
    expect(calls.remove).not.toHaveBeenCalled();
    await userEvent.click(within(table).getByRole('button', { name: i18n.t('adminArtifacts.delete') }));
    confirm = screen.getByRole('alertdialog');
    calls.get.mockResolvedValue(answer({ items: [], next_cursor: null }));
    await userEvent.click(within(confirm).getByRole('button', { name: i18n.t('adminArtifacts.delete') }));
    await waitFor(() => expect(calls.remove).toHaveBeenCalledExactlyOnceWith('/uploads/blobs/{hash}', { params: { path: { hash: 'a'.repeat(64) } } }));
    expect(await screen.findByText(i18n.t('adminArtifacts.noUploads'))).toBeVisible();
  });

  it(`partial/expired custody reports bytes and never pretends it can be opened (${locale})`, async () => {
    await setLocale(locale);
    const partial = reachUpload({ kind: 'machine', state: 'partial', received: 128, content_id: null, can_open: false, can_delete: false });
    show(<><UploadState row={partial} /><UploadOpen row={partial} admin /><UploadAttachments row={partial} /></>);
    expect(screen.getByText(i18n.t('adminArtifacts.uploadStates.partial'))).toBeVisible();
    expect(screen.getByText(/128\s?B\s\/\s512\s?B/u)).toBeVisible();
    expect(screen.getByText(i18n.t('adminArtifacts.unattached'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('adminArtifacts.open') })).not.toBeInTheDocument();
    expect(calls.get).not.toHaveBeenCalled();
  });

  it(`a bounded attachment preview preserves the full count without inventing names (${locale})`, async () => {
    await setLocale(locale);
    show(<UploadAttachments row={reachUpload({ attachment_count: 3, attached_to: [{ kind: 'future-kind', id: 'opaque-private-id' }] })} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('adminArtifacts.attached', { number: '3' }) }));
    expect(await screen.findByText(i18n.t('adminArtifacts.moreReferences', { number: '2' }))).toBeVisible();
    expect(screen.getByText(i18n.t('adminArtifacts.reference'))).toBeVisible();
    expect(screen.queryByText('opaque-private-id')).not.toBeInTheDocument();
  });

  it(`environment snapshots stay cold until disclosed and retain partial facts and maker (${locale})`, async () => {
    await setLocale(locale);
    calls.get.mockResolvedValue(answer({ items: [reachSnapshot({ state: 'partial', missing_count: 2 })], next_cursor: null }));
    show(<QueryClientProvider client={cache()}><EnvironmentSnapshots id={RUNS_ON.workspace} /></QueryClientProvider>);
    expect(calls.get).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText(i18n.t('adminArtifacts.snapshots'), { selector: 'summary' }));
    const table = await screen.findByRole('table', { name: i18n.t('adminArtifacts.snapshots') });
    expect(table).toHaveTextContent('Workspace snapshot'); expect(table).toHaveTextContent(reachPerson.display_name);
    expect(table).toHaveTextContent(i18n.t('adminArtifacts.snapshotStates.partial'));
    expect(table).toHaveTextContent(i18n.t('adminArtifacts.missingFiles', { number: '2' }));
    expect(within(table).getByRole('button', { name: i18n.t('temporary.download') })).toBeVisible();
    expect(calls.get.mock.calls[0]?.[1].params?.path).toEqual({ id: RUNS_ON.workspace });
  });

  it(`export Open shows only the recorded manifest, not the request or sealed parts (${locale})`, async () => {
    await setLocale(locale);
    calls.get.mockResolvedValue(answer({ ...reachExport(), manifest: { made_by: 'Recorded manifest' }, opaque_sealed_part: 'NEVER DISPLAY ME' }));
    show(<QueryClientProvider client={cache()}><ArtifactOpen artifact={{ id: reachExport().id, kind: 'export' }} admin /></QueryClientProvider>);
    expect(calls.get).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('adminArtifacts.open') }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('storage.kinds.export') });
    expect(await within(dialog).findByText(/Recorded manifest/u)).toBeVisible();
    expect(dialog).not.toHaveTextContent('NEVER DISPLAY ME');
    expect(calls.get.mock.calls[0]?.[1].params?.path).toEqual({ id: reachExport().id });
  });

  it(`administrative import inspection reuses the report and offers no owner-only Undo (${locale})`, async () => {
    await setLocale(locale);
    show(<QueryClientProvider client={cache()}><ArtifactOpen artifact={{ id: reachImport().id, kind: 'import' }} admin /></QueryClientProvider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('adminArtifacts.open') }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('storage.kinds.import') });
    expect(await within(dialog).findByText('Reviewed page')).toBeVisible();
    expect(within(dialog).queryByRole('button', { name: i18n.t('importWizard.undo.action') })).not.toBeInTheDocument();
    expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/imports/{id}', '/imports/{id}/report']);
  });
}

it('a missing export manifest stays explicitly unavailable instead of reconstructing one', async () => {
  calls.get.mockResolvedValue(answer(reachExport({ manifest: null, state: 'pending' })));
  show(<QueryClientProvider client={cache()}><ArtifactOpen artifact={{ id: reachExport().id, kind: 'export' }} admin /></QueryClientProvider>);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('adminArtifacts.open') }));
  expect(await screen.findByText(i18n.t('adminArtifacts.noManifest'))).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('temporary.download') })).not.toBeInTheDocument();
});
