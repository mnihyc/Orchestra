/** Native root metadata keeps a department move separate from taking an object to another org. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Homes from '@/api/resourceHome';
import type * as Nodes from '@/api/nodes';
import type * as Destinations from '@/api/copyDestinations';
import type * as Library from '@/api/resourceLibrary';
import { ApiError, client } from '@/api/client';
import { MoveHomeDialog } from '@/features/duplicate/MoveHomeDialog';
import { i18n, setLocale } from '@/i18n';
import { org, me, userRef } from '../doubles/fixtures';
import { treeNode } from '../doubles/tree-node';
import { show } from './support';

const calls = vi.hoisted(() => ({ judge: vi.fn<typeof Destinations.judgeCopyDestinations>(), move: vi.fn<typeof Homes.moveResourceHome>(), tree: vi.fn<typeof Nodes.fetchTree>(), current: vi.fn<() => boolean>() }));
vi.mock('@/api/resourceHome', () => ({ moveResourceHome: calls.move }));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(), sameLibraryReader: () => calls.current() }));
vi.mock('@/api/copyDestinations', async (original) => ({ ...await original<typeof Destinations>(),
  copyDestinationsQuery: (request: Destinations.CopyDestinationsRequest, reader: readonly unknown[]) => ({ queryKey: ['copy-destinations', reader, request], queryFn: ({ signal }: { signal: AbortSignal }) => calls.judge(request, signal), retry: false, gcTime: 0 }),
}));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), fetchTree: calls.tree,
  orgsQuery: { queryKey: ['home-orgs'], queryFn: () => Promise.resolve([org('root', 'team', 'Team', userRef(me(1, 'reader', 'Reader'))), org('other-root', 'other', 'Other org', userRef(me(1, 'reader', 'Reader')))]) } }));
const previous = client.session.getState();
const target = { kind: 'provider', id: 'provider', name: 'Provider', rootOrgId: 'root', revision: 7 } as const;
beforeEach(() => {
  vi.clearAllMocks();
  calls.judge.mockImplementation((request) => Promise.resolve({ subject: request.subject, intent: request.intent, source_owned: false,
    destinations: request.destinations.map((destination) => ({ destination, cross_root: destination.kind === 'node' && destination.id.startsWith('other'),
      outcome: request.intent === 'move' ? { state: 'blocked' as const, cause: 'owners_only' as const, problem: { type: 'orch:error:authz.denied', code: 'authz.denied' as const, status: 403, title: 'Denied', detail: '', request_id: 'judge' } }
        : destination.kind === 'node' && destination.id.endsWith('-topic') ? { state: 'blocked' as const, cause: 'destination_creation' as const, problem: { type: 'orch:error:authz.denied', code: 'authz.denied' as const, status: 403, title: 'Denied', detail: '', request_id: 'judge' } } : { state: 'allowed' as const } })) }));
 client.session.setState({ status: 'signed_in', userId: 'home-reader', readerVersion: 940 }); calls.current.mockReturnValue(true);
  calls.move.mockResolvedValue({ projection: 'receipt', kind: 'provider', object_id: 'provider', home_id: 'department', changed: true });
  calls.tree.mockImplementation((id) => Promise.resolve({ nodes: [treeNode({ id, kind: 'org', parent_id: null, name: id === 'root' ? 'Team' : 'Other org' }), ...(id === 'root' ? [treeNode({ id: 'department', kind: 'org', parent_id: 'root', name: 'Department' })] : [])], hidden: [], seq: 1, since: null, complete: true }));
});
afterEach(async () => { cleanup(); client.session.setState(previous); await setLocale('en'); });
async function chooseDepartment(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Team / Department' }));
}
for (const locale of ['en', 'zh-CN'] as const) {
  it(`moves to a department under the same native root in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); const close = vi.fn(); show(<MoveHomeDialog target={target} onClose={close} />);
    await chooseDepartment(user); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveConfirm') }));
    await waitFor(() => expect(calls.move).toHaveBeenCalledExactlyOnceWith('provider', 'provider', { home_id: 'department', expected_revision: 7 }, expect.any(String)));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
  });
  it(`keeps other orgs disabled with a cause and cancellation sends nothing in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<MoveHomeDialog target={target} onClose={() => undefined} />);
    await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
    const other = await screen.findByRole('option', { name: 'Other org' }); expect(other).toHaveAttribute('aria-disabled', 'true'); expect(other).toHaveTextContent(i18n.t('duplicate.causes.owners_only'));
    await user.keyboard('{Escape}'); await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') })); expect(calls.move).not.toHaveBeenCalled();
  });
  it(`keeps a native refusal and selected home in ${locale}`, async () => {
    await setLocale(locale); calls.move.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Denied', detail: '', request_id: 'home-request' }, null));
    const user = userEvent.setup(); show(<MoveHomeDialog target={target} onClose={() => undefined} />); await chooseDepartment(user);
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveConfirm') })); await screen.findByRole('alert');
    expect(screen.getByText('Team / Department', { exact: true })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveConfirm') })); await waitFor(() => expect(calls.move).toHaveBeenCalledTimes(2));
    expect(calls.move.mock.calls[0]?.[3]).toBe(calls.move.mock.calls[1]?.[3]);
  });
}
it('a late home receipt does not close or report success under a different reader', async () => {
  let finish: ((value: Awaited<ReturnType<typeof Homes.moveResourceHome>>) => void) | undefined;
  calls.move.mockImplementation(() => new Promise((resolve) => { finish = resolve; })); const close = vi.fn(); const user = userEvent.setup();
  show(<MoveHomeDialog target={target} onClose={close} />); await chooseDepartment(user); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveConfirm') }));
  await waitFor(() => expect(calls.move).toHaveBeenCalledOnce()); calls.current.mockReturnValue(false);
  await act(async () => { finish?.({ projection: 'receipt', kind: 'provider', object_id: 'provider', home_id: 'department', changed: true }); await Promise.resolve(); });
  expect(close).not.toHaveBeenCalled(); expect(screen.queryByText(i18n.t('duplicate.homeSaved'))).toBeNull();
});
