/** A selected-place review shows all plans before guarded writes, partial results, explicit reads and exact Undo. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { applyArchivePlan, deleteNode, moveNode, previewArchive, previewDelete, previewMove, readNode } from '@/api/nodes';
import { PlaceReview } from '@/features/project/PlaceReview';
import type { Node, NodeArchivePlan, NodeArchiveResult, NodeDeletePlan, NodeMovePlan } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), readNode: vi.fn(), previewArchive: vi.fn(), applyArchivePlan: vi.fn(), previewMove: vi.fn(), moveNode: vi.fn(), previewDelete: vi.fn(), deleteNode: vi.fn() }));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabels: () => new Map<string, string>(), useNodeLabel: (_id: string, name: string | undefined) => name }));
vi.mock('@/features/access/names', () => ({ usePermissionName: () => (name: string) => name }));
vi.mock('@/features/settings/PlacePicker', () => ({ PlacePicker: ({ label, onChange }: { label: string; onChange: (place: { id: string; kind: 'project'; path: string; folder: boolean; color: null }) => void }) => <button onClick={() => onChange({ id: 'destination', kind: 'project', path: 'Acme / Docs', folder: false, color: null })}>{label}</button> }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: React.ReactNode }) => <a href="#place">{children}</a> }));

const place = (id: string): Node => ({ id, kind: 'topic', name: id, org_id: 'acme', revision: 4, archived: false, path: [{ id, kind: 'topic', name: id, redacted: false }], access: { permissions: ['topic.manage', 'topic.archive'] } } as Node);
const ref = (id: string) => ({ id, kind: 'topic' as const, name: id, redacted: false });
function archive(id: string, archiving = true): NodeArchivePlan {
  return { preflight_id: `review-${id}-${String(archiving)}`, stamp: `stamp-${id}`, revision: 4, expires_at: '2099-01-01T00:00:00Z', archiving, node: ref(id), subtree: [{ node: ref(id), revision: 4, own_archived: false, read_only_after: archiving }], counts: { orgs: 0, projects: 0, topics: 1, threads: 1, documents: 2 }, work: [{ thread: { ...ref(`${id}-thread`), kind: 'thread' }, turns: 1, inputs: 2, approvals: 1, leases: 1, session: true, links: 2 }], environments: [{ id: 'env', name: 'Working files', redacted: false }], copies_kept: 1, live_shares_kept: 2, blockers: [] };
}
const refused = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Permission changed', detail: 'This row no longer permits archive.', status: 403, request_id: '' }, null);
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(readNode).mockImplementation((id) => Promise.resolve(place(id)));
  vi.mocked(previewArchive).mockImplementation((id, direction) => Promise.resolve(archive(id, direction)));
});

it('shows every consequence first, applies sequentially, keeps a lost answer uncertain, and undoes only a confirmed success', async () => {
  let release: ((result: NodeArchiveResult) => void) | undefined;
  const first = new Promise<NodeArchiveResult>((resolve) => { release = resolve; });
  vi.mocked(applyArchivePlan).mockImplementation((plan) => plan.node.id === 'First' && plan.archiving ? first : plan.node.id === 'Refused' ? Promise.reject(refused) : plan.node.id === 'Lost' ? Promise.reject(new Error('The answer was lost.')) : Promise.resolve({ node: place(plan.node.id), undo: null }));
  show(<PlaceReview action="archive" rows={['First', 'Refused', 'Lost'].map(place)} onClose={vi.fn()} onApplied={vi.fn()} />);
  await screen.findByRole('button', { name: 'Apply to 3 ready places' });
  expect(screen.getAllByRole('list', { name: 'Work that stops' })).toHaveLength(3);
  expect(screen.getAllByText('Working files')).toHaveLength(3);
  expect(screen.getAllByText(/existing copy stays/)).toHaveLength(3);
  expect(applyArchivePlan).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Apply to 3 ready places' }));
  expect(screen.getByText('Applying')).toBeVisible();
  expect(applyArchivePlan).toHaveBeenCalledTimes(1);
  const undo = archive('First', false);
  await act(async () => { release?.({ node: { ...place('First'), revision: 5, archived: true }, undo }); await first; });
  expect(await screen.findByText('Answer lost; outcome unknown')).toBeVisible();
  expect(within(screen.getByRole('heading', { name: 'Refused' }).closest('li')!).getByText('Refused', { selector: 'p', exact: true })).toBeVisible();
  expect(screen.getByText('Some rows did not succeed. Each result below stands on its own.')).toBeVisible();
  const lost = screen.getByRole('heading', { name: 'Lost' }).closest('li')!;
  vi.mocked(readNode).mockResolvedValueOnce({ ...place('Lost'), revision: 5, archived: true });
  await userEvent.click(within(lost).getByRole('button', { name: 'Read current state' }));
  expect(await within(lost).findByText(/Current archive state/)).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Undo successful rows' }));
  await waitFor(() => expect(applyArchivePlan).toHaveBeenLastCalledWith(undo));
  expect(vi.mocked(applyArchivePlan).mock.calls.filter(([plan]) => !plan.archiving).map(([plan]) => plan.node.id)).toEqual(['First']);
});

it('keeps a blocker visible and never applies that row', async () => {
  vi.mocked(previewArchive).mockResolvedValue({ ...archive('Blocked'), blockers: [{ code: 'thread.side', message: 'This side thread is closed with its main thread.', missing: null }] });
  show(<PlaceReview action="archive" rows={[place('Blocked')]} onClose={vi.fn()} onApplied={vi.fn()} />);
  expect(await screen.findByText(i18n.t('errors.thread.side'))).toBeVisible();
  expect(screen.queryByText('thread.side', { exact: true })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Apply to 0 ready places' })).toBeDisabled();
  expect(applyArchivePlan).not.toHaveBeenCalled();
});

it('reviews unarchive separately and says that settled work will not restart', async () => {
  show(<PlaceReview action="unarchive" rows={[place('Shelved')]} onClose={vi.fn()} onApplied={vi.fn()} />);
  expect(await screen.findByText('Unarchive restarts no turn, input, approval, lease or harness session.')).toBeVisible();
  expect(previewArchive).toHaveBeenCalledWith('Shelved', false);
  expect(applyArchivePlan).not.toHaveBeenCalled();
});

it('sends the displayed move review and its revision instead of rereading a newer one while applying', async () => {
  const plan: NodeMovePlan = { preflight_id: 'move-reviewed', stamp: 'subtree-and-effects', revision: 7, expires_at: '2099-01-01T00:00:00Z', node: ref('Move me'), from: { ...ref('source'), kind: 'project' }, to: { ...ref('destination'), kind: 'project' }, access: { pages: 2, orgs: null, gaining: [], losing: [], more: 0 }, roles: { gaining: 1, losing: 2 }, addresses: { members: 0, resources: 0 }, settings: [{ key: 'chat.enabled', from: true, to: false, locked_by: null }], blockers: [] };
  vi.mocked(previewMove).mockResolvedValue(plan);
  vi.mocked(moveNode).mockResolvedValue({ ...place('Move me'), revision: 8 });
  show(<PlaceReview action="move" rows={[place('Move me')]} onClose={vi.fn()} onApplied={vi.fn()} />);
  await userEvent.click(screen.getByRole('button', { name: 'Move to' }));
  expect(await screen.findByText('Inherited roles: 1 people gain, 2 lose.')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Apply to 1 ready place' }));
  await waitFor(() => expect(moveNode).toHaveBeenCalledWith('topic', 'Move me', 'destination', 7, plan, expect.any(String)));
});

it('shows the deletion group and last recovery day before sending each deletion fence', async () => {
  const plan: NodeDeletePlan = { preflight_id: 'delete-reviewed', revision: 4, expires_at: '2099-01-01T00:00:00Z', node: ref('Delete me'), counts: { orgs: 0, projects: 0, topics: 1, threads: 0, documents: 2 }, purge_at: '2099-02-01T00:00:00Z', restore_by: { permission: 'project.manage', place: { ...ref('source'), kind: 'project' } } };
  vi.mocked(previewDelete).mockResolvedValue(plan);
  vi.mocked(deleteNode).mockResolvedValue({ deletion_id: 'exact-deletion' });
  show(<PlaceReview action="delete" rows={[place('Delete me')]} onClose={vi.fn()} onApplied={vi.fn()} />);
  expect(await screen.findByText('Who may bring it back')).toBeVisible();
  expect(deleteNode).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Apply to 1 ready place' }));
  await waitFor(() => expect(deleteNode).toHaveBeenCalledWith('Delete me', { preflight_id: 'delete-reviewed', expected_revision: 4 }, expect.any(String)));
});
