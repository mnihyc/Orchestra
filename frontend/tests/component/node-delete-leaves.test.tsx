/** Delete moves a place to the trash at once: no question, no plan read first; the line after it says what running work
 * stopped and offers Show and Undo, and a screen that lay in the place leaves for the place above. */
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { deleteNode, previewDelete, readNode, restoreNode } from '@/api/nodes';
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

it('moves the place to the trash at once, asking nothing and reading no plan first', async () => {
  remove();
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(1));
  expect(deleteNode).toHaveBeenCalledWith(ACME.upgradeDeps, {}, expect.any(String));
  expect(previewDelete).not.toHaveBeenCalled();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(await screen.findByText('Moved “Upgrade deps” to Trash')).toBeInTheDocument();
});

it('its Undo restores exactly that deletion, and Show opens the trash of the place above', async () => {
  remove();
  await screen.findByText('Moved “Upgrade deps” to Trash');
  await userEvent.click(screen.getByRole('button', { name: 'Show' }));
  expect(useDialogState.getState().current).toMatchObject({ type: 'deleted', node: { id: ACME.web } });
  expect(screen.getByText('Moved “Upgrade deps” to Trash')).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Undo' }));
  await vi.waitFor(() => expect(restoreNode).toHaveBeenCalledWith(ACME.upgradeDeps, 'deletion'));
  expect(await screen.findByText('“Upgrade deps” is back')).toBeInTheDocument();
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
  expect(await screen.findByText('Moved “Upgrade deps” to Trash')).toBeInTheDocument();
  expect(calls.navigate).not.toHaveBeenCalled();
});

it('says the running work it stopped in the same line, without asking', async () => {
  treeWriter.reset(ACME.org, acmeTree().map((row) => row.id === ACME.depsThread && row.thread !== null && row.thread !== undefined ? { ...row, thread: { ...row.thread, state: 'running' as const } } : row), 100);
  remove();
  expect(await screen.findByText('1 running thread stopped')).toBeInTheDocument();
});

it('names a refusal in the line and offers no Undo for what did not move', async () => {
  vi.mocked(deleteNode).mockRejectedValue(new Error('not allowed here'));
  remove();
  expect(await screen.findByText('“Upgrade deps” was not moved to Trash')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
});
