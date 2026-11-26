/** Delete moves a place to the trash at once: no question, no plan read first; the line after it says what running work
 * stopped and offers Show and Undo, and a screen that lay in the place leaves for the place above. */
import { act, cleanup, screen, within } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { deleteNode, nodeKey, previewDelete, readNode, restoreNode } from '@/api/nodes';
import { NodeDialogHost } from '@/features/navigation/NodeDialogHost';
import { openNodeDialog, useDialogState } from '@/features/navigation/NodeDialogs';
import type { Node } from '@/generated/api-types';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ navigate: vi.fn(), viewing: { value: undefined as string | undefined } }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), readNode: vi.fn(), previewDelete: vi.fn(), deleteNode: vi.fn(), restoreNode: vi.fn() }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<object>(), useCurrentHeading: () => (calls.viewing.value === undefined ? null : { title: 'Upgrade deps', nodeId: calls.viewing.value }) }));

const place = { id: ACME.upgradeDeps, kind: 'topic' as const, name: 'Upgrade deps', parentId: ACME.web, archived: false, status: null, folder: false };

beforeEach(() => {
  vi.clearAllMocks();
  treeWriter.reset(ACME.org, acmeTree(), 100);
  vi.mocked(readNode).mockResolvedValue({ id: place.id, kind: 'topic', name: place.name, org_id: ACME.org, revision: 1, archived: false, path: [], access: { permissions: ['topic.manage'] } } as unknown as Node);
  // As the answer to the deletion does: the tree has dropped the place and what lay in it before the caller goes on.
  vi.mocked(deleteNode).mockImplementation(() => {
    treeWriter.reset(ACME.org, acmeTree().filter((row) => row.id !== ACME.upgradeDeps && row.id !== ACME.depsThread), 101);
    return Promise.resolve({ deletion_id: 'deletion' });
  });
  vi.mocked(restoreNode).mockResolvedValue({ id: place.id } as unknown as Node);
});
afterEach(() => { cleanup(); useTreeStore.setState({ byOrg: new Map() }); useDialogState.setState({ current: null, open: false }); calls.viewing.value = undefined; });

function remove(): void {
  show(<NodeDialogHost />);
  act(() => openNodeDialog({ type: 'delete', node: place }));
}

/** The newest notice that says `text`: notices of earlier cases stay in the one shared list, and a screen reader's copy has no item. */
async function notice(text: string): Promise<HTMLElement> {
  const item = (await screen.findAllByText(text)).map((one) => one.closest('li')).filter((one) => one !== null).at(-1);
  if (item === undefined) throw new Error(`no notice says ${text}`);
  return item;
}

it('moves the place to the trash at once, asking nothing and reading no plan first', async () => {
  remove();
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(1));
  expect(deleteNode).toHaveBeenCalledWith(ACME.upgradeDeps, {}, expect.any(String));
  expect(previewDelete).not.toHaveBeenCalled();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(await notice('Moved “Upgrade deps” to Trash')).toBeInTheDocument();
});

it('its Undo restores exactly that deletion, and Show opens the trash of the place above', async () => {
  remove();
  const line = await notice('Moved “Upgrade deps” to Trash');
  await userEvent.click(within(line).getByRole('button', { name: 'Show' }));
  expect(useDialogState.getState().current).toMatchObject({ type: 'deleted', node: { id: ACME.web } });
  // Notices wait while a dialog is open: Show keeps the line, which is back once the trash is closed.
  act(() => useDialogState.setState({ current: null, open: false }));
  const kept = await notice('Moved “Upgrade deps” to Trash');
  await userEvent.click(within(kept).getByRole('button', { name: 'Undo' }));
  await vi.waitFor(() => expect(restoreNode).toHaveBeenCalledWith(ACME.upgradeDeps, 'deletion'));
  expect(await notice('“Upgrade deps” is back')).toBeInTheDocument();
});

it('takes a screen that lay in the place deleted to the place above it, once the answer has taken the place out of the tree', async () => {
  calls.viewing.value = ACME.depsThread;
  remove();
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(1));
  await vi.waitFor(() => expect(calls.navigate).toHaveBeenCalledWith({ to: `/project/${ACME.web}` }));
});

it('leaves a screen that lay elsewhere where it is', async () => {
  calls.viewing.value = ACME.fixLogin;
  remove();
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(1));
  expect(await notice('Moved “Upgrade deps” to Trash')).toBeInTheDocument();
  expect(calls.navigate).not.toHaveBeenCalled();
});

it('says the running work it stopped in the same line, without asking', async () => {
  treeWriter.reset(ACME.org, acmeTree().map((row) => row.id === ACME.depsThread && row.thread !== null && row.thread !== undefined ? { ...row, thread: { ...row.thread, state: 'running' as const } } : row), 100);
  remove();
  expect(await notice('1 running thread stopped')).toHaveTextContent('Moved “Upgrade deps” to Trash');
});

it('names a refusal in the line and offers no Undo for what did not move', async () => {
  vi.mocked(deleteNode).mockRejectedValue(new Error('not allowed here'));
  remove();
  const line = await notice('“Upgrade deps” was not moved to Trash');
  expect(within(line).queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
  expect(within(line).queryByRole('button', { name: 'Show' })).not.toBeInTheDocument();
});

it('an org that lies in another keeps its dialog, for where its members and resources go', async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY }, mutations: { retry: false } } });
  client.setQueryData(nodeKey('department'), { id: 'department', kind: 'org', name: 'Research', parent_id: ACME.org, org_id: ACME.org, org_kind: 'team', revision: 1, archived: false, path: [], member_count: 2, access: { permissions: ['org.manage'] } } as unknown as Node);
  vi.mocked(previewDelete).mockReturnValue(new Promise(() => undefined));
  show(<NodeDialogHost />, client);
  // Named by a favorite, the place does not know its parent: the org itself says it lies in another.
  act(() => openNodeDialog({ type: 'delete', node: { id: 'department', kind: 'org', name: 'Research', parentId: null, archived: false, status: null, folder: false } }));
  expect(await screen.findByRole('alertdialog')).toBeInTheDocument();
  expect(deleteNode).not.toHaveBeenCalled();
});
