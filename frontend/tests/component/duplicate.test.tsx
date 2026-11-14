/** The existing dialog, picker and people card keep each native copy as one deliberate act. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useEffect } from 'react';
import { useQueryClient, type QueryClient } from '@tanstack/react-query';
import type * as Loss from '@/api/loss';
import { ADA } from '../unit/general-access-fixtures';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import type * as Copies from '@/api/duplicates';
import type * as Nodes from '@/api/nodes';
import type * as Destinations from '@/api/copyDestinations';
import type * as Library from '@/api/resourceLibrary';
import { ApiError, client } from '@/api/client';
import { DuplicateDialog } from '@/features/duplicate/DuplicateDialog';
import { DuplicateHost } from '@/features/duplicate/DuplicateHost';
import { CopySetupResult } from '@/features/duplicate/CopySetup';
import { openDuplicate, useDuplicateHost } from '@/features/duplicate/target';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { org, me, userRef } from '../doubles/fixtures';
import { treeNode } from '../doubles/tree-node';
import { show } from './support';

const calls = vi.hoisted(() => ({ judge: vi.fn<typeof Destinations.judgeCopyDestinations>(), copy: vi.fn<typeof Copies.duplicate>(), document: vi.fn<typeof Copies.duplicateDocument>(),
  preview: vi.fn<typeof Copies.previewChatDuplicate>(), recipients: vi.fn<typeof Loss.moreOperationRecipients>(), tree: vi.fn<typeof Nodes.fetchTree>(), navigate: vi.fn<(options: { href: string }) => Promise<void>>(), current: vi.fn<() => boolean>() }));
vi.mock('@/api/loss', async (original) => ({ ...await original<typeof Loss>(), moreOperationRecipients: calls.recipients }));
vi.mock('@/api/duplicates', async (original) => ({ ...await original<typeof Copies>(), duplicate: calls.copy, duplicateDocument: calls.document, previewChatDuplicate: calls.preview }));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(), sameLibraryReader: () => calls.current() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/copyDestinations', async (original) => ({ ...await original<typeof Destinations>(),
  copyDestinationsQuery: (request: Destinations.CopyDestinationsRequest, reader: readonly unknown[]) => ({ queryKey: ['copy-destinations', reader, request], queryFn: ({ signal }: { signal: AbortSignal }) => calls.judge(request, signal), retry: false, gcTime: 0 }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), fetchTree: calls.tree,
  orgsQuery: { queryKey: ['copy-orgs'], queryFn: () => Promise.resolve([org('source-org', 'source', 'Source', userRef(me(1, 'reader', 'Reader'))), org('other-org', 'other', 'Other org', userRef(me(1, 'reader', 'Reader')))]) } }));
const previous = client.session.getState();
beforeEach(() => {
  vi.clearAllMocks();
  calls.judge.mockImplementation((request) => Promise.resolve({ subject: request.subject, intent: request.intent, source_owned: false,
    destinations: request.destinations.map((destination) => ({ destination, cross_root: destination.kind === 'node' && destination.id.startsWith('other'),
      outcome: request.intent === 'move' ? { state: 'blocked' as const, cause: 'owners_only' as const, problem: { type: 'orch:error:authz.denied', code: 'authz.denied' as const, status: 403, title: 'Denied', detail: '', request_id: 'judge' } }
        : destination.kind === 'node' && destination.id.endsWith('-topic') ? { state: 'blocked' as const, cause: 'destination_creation' as const, problem: { type: 'orch:error:authz.denied', code: 'authz.denied' as const, status: 403, title: 'Denied', detail: '', request_id: 'judge' } } : { state: 'allowed' as const } })) }));
 useDuplicateHost.getState().close();
  client.session.setState({ status: 'signed_in', userId: 'copy-reader', readerVersion: 910 });
  calls.current.mockReturnValue(true); calls.navigate.mockResolvedValue(undefined);
  calls.copy.mockImplementation((kind) => Promise.resolve({ projection: 'receipt', kind, id: 'fresh-copy' }));
  calls.document.mockResolvedValue({ kind: 'document', id: 'fresh-document' });
  calls.preview.mockResolvedValue({ people: 3, notify_limit: 200, preview_id: 'audience-preview' });
  calls.recipients.mockResolvedValue({ people: { items: [ADA], next_cursor: null }, hidden: 2, total: 3 });
  calls.tree.mockImplementation((id) => Promise.resolve({ nodes: [treeNode({ id, kind: 'org', parent_id: null, name: id === 'source-org' ? 'Source' : 'Other org' }), treeNode({ id: `${id}-topic`, kind: 'topic', parent_id: id, name: 'A topic' })], hidden: [], seq: 1, next_cursor: null, complete: true, since: null }));
});
afterEach(async () => { cleanup(); useDuplicateHost.getState().close(); client.session.setState(previous); await setLocale('en'); });
const target = { kind: 'provider', id: 'source', name: 'Provider', revision: 7 } as const;
async function chooseOther(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Other org' }));
}
for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a missing native setup observation unknown after a completed copy (${locale})`, async () => {
    await setLocale(locale);
    show(<><DuplicateDialog target={{ ...target, setup: [] }} quick onClose={() => undefined} /><CopySetupResult kind="provider" id="fresh-copy" /></>);
    expect(await screen.findByText(i18n.t('duplicate.setup.unknown'))).toBeVisible();
    expect(screen.queryByText(i18n.t('duplicate.setup.after.credentials_not_copied'))).toBeNull();
  });
  it(`keeps a known empty native setup observation separate from an unknown one (${locale})`, async () => {
    await setLocale(locale);
    calls.copy.mockResolvedValue({ projection: 'receipt', kind: 'provider', id: 'fresh-copy', setup: [] });
    const view = show(<><DuplicateDialog target={{ ...target, setup: [] }} quick onClose={() => undefined} /><CopySetupResult kind="provider" id="fresh-copy" /></>);
    await waitFor(() => expect(calls.navigate).toHaveBeenCalledOnce());
    expect(view.container.querySelector('[data-copy-setup]')).toBeNull();
  });
  it(`shows native copy setup before acting and the actual accepted setup in its result inspector (${locale})`, async () => {
    await setLocale(locale);
    let finish: ((receipt: Copies.DuplicateReceipt) => void) | undefined;
    calls.copy.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const setup = ['sign_in_not_copied', 'source_consent_reset'] as const;
    show(<><DuplicateDialog target={{ ...target, setup }} quick onClose={() => undefined} /><CopySetupResult kind="provider" id="fresh-copy" /></>);
    await waitFor(() => expect(calls.copy).toHaveBeenCalledOnce());
    for (const note of setup) expect(screen.getByText(new RegExp(i18n.t(`duplicate.setup.before.${note}`)))).toBeVisible();
    expect(screen.queryByText(i18n.t('duplicate.setup.after.sign_in_not_copied'), { exact: false })).toBeNull();
    await act(async () => { finish?.({ projection: 'receipt', kind: 'provider', id: 'fresh-copy', setup: ['credentials_not_copied'] }); await Promise.resolve(); });
    expect(await screen.findByText(i18n.t('duplicate.setup.after.credentials_not_copied'))).toBeVisible();
    expect(screen.queryByText(i18n.t('duplicate.setup.after.sign_in_not_copied'), { exact: false })).toBeNull();
    act(() => client.session.setState({ readerVersion: 911 }));
    await waitFor(() => expect(screen.queryByText(i18n.t('duplicate.setup.after.credentials_not_copied'))).toBeNull());
  });
  it(`one click accepts a sparse receipt and opens its identity in ${locale}`, async () => {
    await setLocale(locale); show(<DuplicateDialog target={target} quick onClose={() => undefined} />);
    await waitFor(() => expect(calls.copy).toHaveBeenCalledExactlyOnceWith('provider', 'source', { expected_revision: 7 }, expect.any(String), {}));
    await waitFor(() => expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ href: '/resources/providers/fresh-copy' }));
  });
  it(`cancels a destination dialog without making a copy in ${locale}`, async () => {
    await setLocale(locale); const close = vi.fn(); const user = userEvent.setup(); show(<DuplicateDialog target={target} onClose={close} />);
    await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
    expect(close).toHaveBeenCalledOnce(); expect(calls.copy).not.toHaveBeenCalled();
  });
  it(`uses the real cross-org picker, retains incompatible topics and sends only the chosen destination in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<DuplicateDialog target={target} onClose={() => undefined} />);
    await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
    await waitFor(() => {
      const topic = screen.getByRole('option', { name: 'Other org / A topic' });
      expect(topic).toHaveAttribute('aria-disabled', 'true'); expect(topic).toHaveTextContent(i18n.t('duplicate.causes.destination_creation'));
    });
    await user.click(screen.getByRole('option', { name: 'Other org' }));
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
    await waitFor(() => expect(calls.copy).toHaveBeenCalledWith('provider', 'source', { expected_revision: 7, destination: { kind: 'node', id: 'other-org' } }, expect.any(String), {}));
  });
  it(`keeps native source/destination refusal and the chosen place visible in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    calls.copy.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Denied', detail: '', request_id: 'copy-request' }, null));
    show(<DuplicateDialog target={target} onClose={() => undefined} />); await chooseOther(user);
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
    await screen.findByRole('alert'); expect(screen.getByText('Other org', { exact: true })).toBeInTheDocument(); expect(calls.navigate).not.toHaveBeenCalled();
  });
  it(`notifies the native contributor preview only after the explicit choice in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<DuplicateDialog target={{ kind: 'chat', id: 'group', name: 'Design' }} onClose={() => undefined} />);
    await chooseOther(user); const check = screen.getByRole('checkbox'); expect(check).not.toBeChecked();
    await user.click(check); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
    await waitFor(() => expect(calls.copy).toHaveBeenCalledWith('chat', 'group', { destination: { kind: 'node', id: 'other-org' } }, expect.any(String), { notify: { preview: 'audience-preview' } }));
  });
  it(`a notification limit does not prevent a silent native copy in ${locale}`, async () => {
    await setLocale(locale); calls.preview.mockResolvedValue({ people: 201, notify_limit: 200, preview_id: null });
    const user = userEvent.setup(); show(<DuplicateDialog target={{ kind: 'chat', id: 'group', name: 'Design' }} onClose={() => undefined} />);
    await chooseOther(user); expect(screen.getByRole('checkbox')).toBeDisabled();
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
    await waitFor(() => expect(calls.copy).toHaveBeenCalledWith('chat', 'group', { destination: { kind: 'node', id: 'other-org' } }, expect.any(String), {}));
  });
}
it('a lost answer retries the same act key and never borrows source configuration', async () => {
  calls.copy.mockRejectedValueOnce(new NetworkError('failed')); const user = userEvent.setup(); show(<DuplicateDialog target={target} quick onClose={() => undefined} />);
  await user.click(await screen.findByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.copy).toHaveBeenCalledTimes(2));
  expect(calls.copy.mock.calls[0]?.[3]).toBe(calls.copy.mock.calls[1]?.[3]);
});
it('a changed deliberate destination/name gets a different act identity', async () => {
  calls.copy.mockRejectedValue(new NetworkError('failed')); const user = userEvent.setup(); show(<DuplicateDialog target={target} onClose={() => undefined} />);
  const name = screen.getByRole('textbox', { name: new RegExp(i18n.t('duplicate.name')) });
  expect(name.compareDocumentPosition(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') })) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  await chooseOther(user); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
  await screen.findByRole('alert'); await user.type(screen.getByRole('textbox', { name: new RegExp(i18n.t('duplicate.name')) }), 'Separate provider');
  await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
  await waitFor(() => expect(calls.copy).toHaveBeenCalledTimes(2));
  expect(calls.copy.mock.calls[0]?.[3]).not.toBe(calls.copy.mock.calls[1]?.[3]);
});
it('a completed copy is not opened under a replacement reader', async () => {
  let finish: ((value: Copies.DuplicateReceipt) => void) | undefined;
  calls.copy.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  show(<DuplicateDialog target={target} quick onClose={() => undefined} />); await waitFor(() => expect(calls.copy).toHaveBeenCalledOnce());
  calls.current.mockReturnValue(false);
  await act(async () => { finish?.({ projection: 'receipt', kind: 'provider', id: 'fresh-copy' }); await Promise.resolve(); });
  expect(calls.navigate).not.toHaveBeenCalled();
});
it('the shared host retires its source and dialog when the reader changes', async () => {
  show(<DuplicateHost />); act(() => openDuplicate(target));
  await screen.findByRole('dialog'); calls.current.mockReturnValue(false);
  act(() => client.session.setState({ readerVersion: 911 }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull()); expect(useDuplicateHost.getState().current).toBeNull();
});
it('documents use their actual immediate native directory and open the returned identity', async () => {
  show(<DuplicateDialog target={{ kind: 'document', id: 'source-document', name: 'Plan', directory: { kind: 'container', id: 'source-folder' } }} quick onClose={() => undefined} />);
  await waitFor(() => expect(calls.document).toHaveBeenCalledWith('source-document', { kind: 'container', id: 'source-folder' }, undefined, expect.any(String)));
  expect(calls.copy).not.toHaveBeenCalled(); await waitFor(() => expect(calls.navigate.mock.calls.at(-1)?.[0].href).toMatch(/\/doc\/fresh-document$/u));
});
it('an audience read failure leaves the notification choice unavailable while silent copy remains possible', async () => {
  calls.preview.mockRejectedValue(new NetworkError('failed')); const user = userEvent.setup(); show(<DuplicateDialog target={{ kind: 'chat', id: 'group', name: 'Design' }} onClose={() => undefined} />);
  await chooseOther(user); expect(screen.getByRole('checkbox')).toBeDisabled();
  const dialog = screen.getByRole('dialog'); await user.click(within(dialog).getByRole('button', { name: i18n.t('duplicate.now') }));
  await waitFor(() => expect(calls.copy).toHaveBeenCalledWith('chat', 'group', { destination: { kind: 'node', id: 'other-org' } }, expect.any(String), {}));
});

const held: { cache: QueryClient | null } = { cache: null };
function CurrentCache() {
  const cache = useQueryClient();
  useEffect(() => { held.cache = cache; return () => { held.cache = null; }; }, [cache]);
  return null;
}
it('a contributor preview refresh cannot turn retry into a second copy with a different key', async () => {
  calls.copy.mockRejectedValueOnce(new NetworkError('failed'));
  const user = userEvent.setup(); show(<><CurrentCache /><DuplicateDialog target={{ kind: 'chat', id: 'group', name: 'Design' }} onClose={() => undefined} /></>);
  await chooseOther(user); await user.click(screen.getByRole('checkbox')); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
  await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  calls.preview.mockResolvedValue({ people: 4, notify_limit: 200, preview_id: 'new-preview' });
  await act(async () => { await held.cache?.invalidateQueries({ queryKey: ['chat-duplicate-audience'] }); });
  await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2));
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.copy).toHaveBeenCalledTimes(2));
  expect(calls.copy.mock.calls[1]).toEqual(calls.copy.mock.calls[0]);
  expect(calls.copy.mock.calls[1]?.[4]).toEqual({ notify: { preview: 'audience-preview' } });
});
it('opens actual native recipient names through the common bounded people list', async () => {
  const user = userEvent.setup(); show(<DuplicateDialog target={{ kind: 'chat', id: 'group', name: 'Design' }} onClose={() => undefined} />);
  const count = await screen.findByRole('button', { name: i18n.t('operation.notify', { count: 3, number: '3' }) });
  await user.click(count); await screen.findByText(ADA.display_name, { exact: true });
  expect(calls.recipients).toHaveBeenCalledWith('audience-preview', undefined, expect.any(AbortSignal));
  expect(calls.copy).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`fulfils the native environment placement choice with destination defaults (${locale})`, async () => {
  await setLocale(locale);
  calls.judge.mockImplementation((request) => Promise.resolve({ subject: request.subject, intent: request.intent, source_owned: false,
    destinations: request.destinations.map((destination) => ({ destination, cross_root: true, outcome: { state: 'needs_choice' as const, choice: 'environment_placement' as const } })) }));
  const user = userEvent.setup(); show(<DuplicateDialog target={{ kind: 'environment', id: 'environment', name: 'Workspace' }} onClose={() => undefined} />);
  await chooseOther(user); await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.placement') }));
  await user.click(screen.getByRole('option', { name: i18n.t('duplicate.managedPlacement') }));
  await user.click(screen.getByRole('button', { name: i18n.t('duplicate.now') }));
  await waitFor(() => expect(calls.copy).toHaveBeenCalledWith('environment', 'environment', { destination: { kind: 'node', id: 'other-org' } }, expect.any(String), { managed: {} }));
});
