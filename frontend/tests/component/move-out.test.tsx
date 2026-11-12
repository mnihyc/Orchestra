/** Destination ownership and deletion effects come from native read-only facts before the one write. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Destinations from '@/api/copyDestinations';
import { ApiError } from '@/api/client';
import type * as Moves from '@/api/moveOut';
import type * as Nodes from '@/api/nodes';
import type * as Resources from '@/api/resources';
import type * as Loss from '@/api/loss';
import type * as Library from '@/api/resourceLibrary';
import type * as Router from '@tanstack/react-router';
import { MoveHomeDialog } from '@/features/duplicate/MoveHomeDialog';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { org, me, userRef } from '../doubles/fixtures';
import { treeNode } from '../doubles/tree-node';
import { show } from './support';
const calls = vi.hoisted(() => ({ preview: vi.fn<typeof Moves.previewMoveOut>(), execute: vi.fn<typeof Moves.executeMoveOut>(), recipients: vi.fn<typeof Loss.moreOperationRecipients>(), same: vi.fn<() => boolean>(), navigate: vi.fn<(options: { href: string }) => Promise<void>>() }));
vi.mock('@/api/moveOut', async (original) => ({ ...await original<typeof Moves>(), previewMoveOut: calls.preview, executeMoveOut: calls.execute }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof Resources>(), machinesQuery: { queryKey: ['move-machines'], queryFn: () => Promise.resolve([{ id: 'receiver', name: 'Receiving machine' }]) } }));
vi.mock('@/api/loss', async (original) => ({ ...await original<typeof Loss>(), moreOperationRecipients: calls.recipients }));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(), sameLibraryReader: () => calls.same() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/copyDestinations', async (original) => ({ ...await original<typeof Destinations>(), copyDestinationsQuery: (request: Destinations.CopyDestinationsRequest) => ({ queryKey: ['copy-destinations', request], queryFn: () => Promise.resolve({ subject: request.subject, intent: request.intent, source_owned: true, destinations: request.destinations.map((destination) => ({ destination, cross_root: true, outcome: { state: 'needs_choice', choice: 'delete_preview' } })) }) }) }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), orgsQuery: { queryKey: ['move-orgs'], queryFn: () => Promise.resolve([org('other-org', 'other', 'Other org', userRef(me(1, 'ada', 'Ada')))]) }, fetchTree: () => Promise.resolve({ seq: 1, since: null, complete: true, hidden: [], nodes: [treeNode({ id: 'other-org', kind: 'org', parent_id: null, name: 'Other org' })] }) }));
const target = { kind: 'agent', id: 'source', name: 'Profile', rootOrgId: 'source-org', revision: 7 } as const;
beforeEach(() => { vi.clearAllMocks(); calls.same.mockReturnValue(true); calls.navigate.mockResolvedValue(undefined); calls.preview.mockResolvedValue({ subject: { kind: 'agent', id: 'source' }, destination: { kind: 'node', id: 'other-org' }, revision: 7, stamp: 'native-footprint', effects: ['remove_source', 'stop_bound_work'] }); calls.execute.mockResolvedValue({ kind: 'agent', id: 'new-copy' }); });
afterEach(async () => { cleanup(); await setLocale('en'); });
async function choose(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.destination') }));
  const option = await screen.findByRole('option', { name: 'Other org' }); await waitFor(() => expect(option).not.toHaveAttribute('aria-disabled', 'true')); await user.click(option);
}
for (const locale of ['en', 'zh-CN'] as const) {
  it(`reviews actual deletion/stop consequences before one cross-org move (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<MoveHomeDialog target={target} onClose={() => undefined} />); await choose(user);
    await screen.findByRole('table', { name: i18n.t('duplicate.deletePlan') }); expect(screen.getByText(i18n.t('duplicate.moveEffects.stop_bound_work'))).toBeInTheDocument(); expect(calls.execute).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) }));
    await waitFor(() => expect(calls.execute).toHaveBeenCalledWith(target, { copy: { destination: { kind: 'node', id: 'other-org' }, expected_revision: 7 }, delete_plan: { revision: 7, stamp: 'native-footprint' } }, expect.any(String)));
    await waitFor(() => expect(calls.navigate.mock.calls.at(-1)?.[0].href).toMatch(/\/resources\/agents\/new-copy$/u));
  });
  it(`cancel sends no move even after a preview (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(), close = vi.fn(); show(<MoveHomeDialog target={target} onClose={close} />); await choose(user); await screen.findByRole('table');
    await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') })); expect(close).toHaveBeenCalledOnce(); expect(calls.execute).not.toHaveBeenCalled();
  });
  it(`a failed native preview keeps the chosen home and makes no move (${locale})`, async () => {
    await setLocale(locale); calls.preview.mockRejectedValueOnce(new NetworkError('failed'));
    const user = userEvent.setup(); show(<MoveHomeDialog target={target} onClose={() => undefined} />); await choose(user);
    await screen.findByRole('alert'); expect(screen.getByText('Other org', { exact: true })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) })).toBeDisabled(); expect(calls.execute).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') })); await screen.findByRole('table'); expect(calls.execute).not.toHaveBeenCalled();
  });
  it(`a stale footprint requires explicit current review before a different act (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<MoveHomeDialog target={target} onClose={() => undefined} />); await choose(user); await screen.findByRole('table');
    calls.execute.mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Stale', detail: '', request_id: 'move-stale' }, null));
    const caption = i18n.t('duplicate.moveAcross', { org: 'Other org' }); await user.click(screen.getByRole('button', { name: caption })); await screen.findByRole('alert');
    calls.preview.mockResolvedValue({ subject: { kind: 'agent', id: 'source' }, destination: { kind: 'node', id: 'other-org' }, revision: 8, stamp: 'new-footprint', effects: ['remove_source'] });
    await user.click(screen.getByRole('button', { name: i18n.t('duplicate.reviewAgain') }));
    await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2)); expect(calls.preview.mock.calls[1]?.[1]).toEqual({ destination: { kind: 'node', id: 'other-org' } });
    await screen.findByRole('table'); expect(calls.execute).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: caption })); await waitFor(() => expect(calls.execute).toHaveBeenCalledTimes(2));
    expect(calls.execute.mock.calls[1]?.[1]).toEqual({ copy: { destination: { kind: 'node', id: 'other-org' }, expected_revision: 8 }, delete_plan: { revision: 8, stamp: 'new-footprint' } });
    expect(calls.execute.mock.calls[1]?.[2]).not.toBe(calls.execute.mock.calls[0]?.[2]);
  });

}
it('a lost move answer retries the same frozen footprint and idempotency key', async () => {
  calls.execute.mockRejectedValueOnce(new NetworkError('failed')); const user = userEvent.setup(); show(<MoveHomeDialog target={target} onClose={() => undefined} />); await choose(user); await screen.findByRole('table');
  const confirm = screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) }); await user.click(confirm); await screen.findByRole('alert'); await user.click(confirm);
  await waitFor(() => expect(calls.execute).toHaveBeenCalledTimes(2)); expect(calls.execute.mock.calls[1]).toEqual(calls.execute.mock.calls[0]);
});
it('a late acknowledgement never opens the moved copy under a different reader', async () => {
  let finish: ((value: Moves.MoveOutReceipt) => void) | undefined;
  calls.execute.mockImplementation(() => new Promise((resolve) => { finish = resolve; })); const user = userEvent.setup(), close = vi.fn(); show(<MoveHomeDialog target={target} onClose={close} />); await choose(user); await screen.findByRole('table');
  await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) })); await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); calls.same.mockReturnValue(false);
  await act(async () => { finish?.({ kind: 'agent', id: 'new-copy' }); await Promise.resolve(); }); expect(close).not.toHaveBeenCalled(); expect(calls.navigate).not.toHaveBeenCalled();
});

const groupTarget = { ...target, kind: 'chat', name: 'Design' } as const;
const groupReview = (preview = 'native-member-review'): Moves.ChatMoveOutPlan => ({ delete_plan: { subject: { kind: 'chat', id: 'source' }, destination: { kind: 'node', id: 'other-org' }, revision: 7, stamp: 'group-ending-footprint', effects: ['end_group_and_history'] }, notification: { people: 2, notify_limit: 200, preview_id: preview } });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`the group ending needs its typed name and binds the checked notice to that review (${locale})`, async () => {
    await setLocale(locale); calls.preview.mockResolvedValue(groupReview()); calls.execute.mockResolvedValue({ kind: 'chat', id: 'new-group' }); const user = userEvent.setup(); show(<MoveHomeDialog target={groupTarget} onClose={() => undefined} />); await choose(user);
    await screen.findByText(i18n.t('duplicate.moveEffects.end_group_and_history')); const button = screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) }); expect(button).toBeDisabled();
    const notice = screen.getByRole('checkbox'); expect(notice).not.toBeChecked(); await user.click(notice);
    await user.type(screen.getByRole('textbox', { name: i18n.t('duplicate.typeName') }), 'Design'); await user.click(button);
    await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[1]).toEqual({ copy: { destination: { kind: 'node', id: 'other-org' }, expected_revision: 7 }, delete_plan: { revision: 7, stamp: 'group-ending-footprint' }, notify: true, notify_preview_id: 'native-member-review' });
    await waitFor(() => expect(calls.navigate.mock.calls.at(-1)?.[0].href).toMatch(/\/chat\/new-group$/u));
  });
  it(`an over-limit group notice stays off while a deliberate silent move remains available (${locale})`, async () => {
    await setLocale(locale); calls.execute.mockResolvedValue({ kind: 'chat', id: 'new-group' }); calls.preview.mockResolvedValue({ ...groupReview(), notification: { people: 201, notify_limit: 200, preview_id: null } }); const user = userEvent.setup(); show(<MoveHomeDialog target={groupTarget} onClose={() => undefined} />); await choose(user); await screen.findByRole('textbox', { name: i18n.t('duplicate.typeName') });
    expect(screen.getByRole('checkbox')).toBeDisabled(); await user.type(screen.getByRole('textbox', { name: i18n.t('duplicate.typeName') }), 'Design'); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) }));
    await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[1]).toMatchObject({ notify: null, notify_preview_id: null });
  });
  it(`an imported MCP moves using its chosen physical recipient and current native footprint (${locale})`, async () => {
    await setLocale(locale); calls.execute.mockResolvedValue({ kind: 'mcp_server', id: 'new-mcp' }); const native = { ...target, kind: 'mcp_server', imported: true } as const;
    calls.preview.mockImplementation((_native, copy) => Promise.resolve({ subject: { kind: 'mcp_server', id: 'source' }, destination: { kind: 'node', id: 'other-org' }, revision: 7, stamp: copy.machine_id === 'receiver' ? 'chosen-machine-footprint' : 'original-machine-footprint', effects: ['remove_source'] }));
    const user = userEvent.setup(); show(<MoveHomeDialog target={native} onClose={() => undefined} />); await choose(user); await screen.findByRole('table'); await user.click(screen.getByRole('combobox', { name: i18n.t('duplicate.machine') })); await user.click(await screen.findByRole('option', { name: 'Receiving machine' }));
    const button = screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) }); await waitFor(() => expect(button).toBeEnabled()); await user.click(button);
    await waitFor(() => expect(calls.execute).toHaveBeenCalledOnce()); expect(calls.execute.mock.calls[0]?.[1]).toEqual({ copy: { destination: { kind: 'node', id: 'other-org' }, expected_revision: 7, machine_id: 'receiver' }, delete_plan: { revision: 7, stamp: 'chosen-machine-footprint' } });
  });
}
it('a group stale review resets both confirmation and notice instead of silently moving with a new audience', async () => {
  calls.preview.mockResolvedValue(groupReview()); calls.execute.mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Stale', detail: '', request_id: 'group-stale' }, null)); const user = userEvent.setup(); show(<MoveHomeDialog target={groupTarget} onClose={() => undefined} />); await choose(user); await screen.findByRole('textbox', { name: i18n.t('duplicate.typeName') });
  await user.click(screen.getByRole('checkbox')); await user.type(screen.getByRole('textbox', { name: i18n.t('duplicate.typeName') }), 'Design'); const button = screen.getByRole('button', { name: i18n.t('duplicate.moveAcross', { org: 'Other org' }) }); await user.click(button); await screen.findByRole('alert');
  calls.preview.mockResolvedValue(groupReview('new-member-review')); await user.click(screen.getByRole('button', { name: i18n.t('duplicate.reviewAgain') })); await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(2)); expect(button).toBeDisabled(); expect(screen.getByRole('textbox', { name: i18n.t('duplicate.typeName') })).toHaveValue(''); expect(screen.getByRole('checkbox')).not.toBeChecked(); expect(calls.execute).toHaveBeenCalledOnce();
});
it('the group notice opens the actual native member review in the shared list', async () => {
  calls.preview.mockResolvedValue(groupReview()); calls.recipients.mockResolvedValue({ total: 2, hidden: 1, people: { items: [userRef(me(3, 'visible-person', 'Visible recipient'))], next_cursor: null } }); const user = userEvent.setup(); show(<MoveHomeDialog target={groupTarget} onClose={() => undefined} />); await choose(user); await screen.findByRole('textbox', { name: i18n.t('duplicate.typeName') });
  await user.click(screen.getByRole('button', { name: i18n.t('operation.notify', { count: 2, number: '2' }) })); await screen.findByText('Visible recipient'); expect(calls.recipients).toHaveBeenCalledWith('native-member-review', undefined, expect.any(AbortSignal)); expect(calls.execute).not.toHaveBeenCalled();
});
