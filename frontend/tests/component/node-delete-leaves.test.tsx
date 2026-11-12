/** A screen that lay in the place deleted leaves for the place above, even though the answer to the deletion has already taken the place out of the tree. */
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { deleteNode, previewDelete, readNode } from '@/api/nodes';
import { NodeDialogHost, openNodeDialog } from '@/features/navigation/NodeDialogs';
import type { Node, NodeDeletePlan } from '@/generated/api-types';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ navigate: vi.fn(), viewing: { value: undefined as string | undefined } }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), readNode: vi.fn(), previewDelete: vi.fn(), deleteNode: vi.fn() }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<object>(), useCurrentHeading: () => (calls.viewing.value === undefined ? null : { title: 'Upgrade deps', nodeId: calls.viewing.value }) }));

const topic = acmeTree().find((row) => row.id === ACME.upgradeDeps);
const place = { id: ACME.upgradeDeps, kind: 'topic' as const, name: 'Upgrade deps', parentId: ACME.web, archived: false, status: null, folder: false };
const plan = (): NodeDeletePlan => ({
  preflight_id: 'reviewed', revision: 1, expires_at: '2099-01-01T00:00:00Z', node: { id: place.id, kind: 'topic', name: place.name, redacted: false },
  counts: { orgs: 0, projects: 0, topics: 1, threads: 1, documents: 0 }, purge_at: '2099-02-01T00:00:00Z',
  restore_by: { permission: 'project.manage', place: { id: ACME.web, kind: 'project', name: 'Web', redacted: false } },
});

beforeEach(() => {
  vi.clearAllMocks();
  treeWriter.reset(ACME.org, acmeTree(), 100);
  vi.mocked(readNode).mockResolvedValue({ id: place.id, kind: 'topic', name: place.name, org_id: ACME.org, revision: 1, archived: false, path: [], access: { permissions: ['topic.manage'] } } as unknown as Node);
  vi.mocked(previewDelete).mockResolvedValue(plan());
  // As the answer to the deletion does: the tree has dropped the place and what lay in it before the caller goes on.
  vi.mocked(deleteNode).mockImplementation(() => {
    treeWriter.reset(ACME.org, acmeTree().filter((row) => row.id !== ACME.upgradeDeps && row.id !== ACME.depsThread), 101);
    return Promise.resolve({ deletion_id: 'deletion' });
  });
});
afterEach(() => { cleanup(); useTreeStore.setState({ byOrg: new Map() }); calls.viewing.value = undefined; });

async function remove(): Promise<void> {
  show(<NodeDialogHost />);
  act(() => openNodeDialog({ type: 'delete', node: place }));
  await userEvent.click(await screen.findByRole('button', { name: 'Delete Upgrade deps' }));
}

it('takes a screen that lay in the place deleted to the place above it, once the answer has taken the place out of the tree', async () => {
  expect(topic).toBeDefined();
  calls.viewing.value = ACME.upgradeDeps;
  await remove();
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(1));
  await vi.waitFor(() => expect(calls.navigate).toHaveBeenCalledWith({ to: `/project/${ACME.web}` }));
});

it('leaves a screen that lay elsewhere where it is', async () => {
  calls.viewing.value = ACME.fixLogin;
  await remove();
  await vi.waitFor(() => expect(deleteNode).toHaveBeenCalledTimes(1));
  expect((await screen.findAllByText('Upgrade deps deleted')).length).toBeGreaterThan(0);
  expect(calls.navigate).not.toHaveBeenCalled();
});
