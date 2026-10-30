/** Selected holder shares are reviewed without writes and preserve confirmed, refused and uncertain outcomes for one guarded Undo. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { ShareSeveralDialog } from '@/features/access/Several';
import { inverseShare } from '@/features/access/shareReview';
import type { Entry, PermissionPage, SharePreview, UserRef } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const mocks = vi.hoisted(() => ({ read: vi.fn<(_target: { id: string }) => Promise<PermissionPage>>(), preview: vi.fn<(_target: { id: string }) => Promise<SharePreview>>(), change: vi.fn<(_target: { id: string }, _body: unknown) => Promise<PermissionPage>>() }));
vi.mock('@/api/access', async (original) => ({ ...await original<object>(), readPage: mocks.read, previewShare: mocks.preview, changePage: mocks.change, pageQuery: (target: { id: string }) => ({ queryKey: ['page', target.id], queryFn: () => mocks.read(target) }) }));
vi.mock('@/features/navigation/placeLabels', () => ({ useNodeLabel: (_id: string, name: string) => name }));
vi.mock('@/features/access/names', () => ({ usePermissionName: () => (name: string) => name }));
vi.mock('@/features/access/holders', () => ({ HolderView: () => <span>Vera Rubin</span> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: () => <span>Vera Rubin</span> }));
vi.mock('@/features/access/Tell', () => ({ useDirectOff: () => true, TellThem: () => null, tellable: () => [], tellThem: vi.fn() }));
vi.mock('@/features/access/candidates', () => ({ useTargetOrg: () => 'acme', useCandidates: () => ({ placePicker: null, groups: [{ id: 'people', label: 'People', options: [{ value: 'vera', label: 'Vera Rubin', text: 'Vera Rubin' }] }], byValue: new Map([['vera', { input: { kind: 'user', id: 'vera' }, holder: { kind: 'user', user: { id: 'vera', display_name: 'Vera Rubin' } }, name: 'Vera Rubin', outside: false }]]), loading: false }) }));

const VERA: UserRef = { id: 'vera', username: 'vera', display_name: 'Vera Rubin', kind: 'human', avatar: { text: 'V', color: 1 } };
const entry = (id: string, revision = 1): Entry => ({ id, holder: { kind: 'user', user: VERA }, revision, read_first: false, options: {}, expires_at: null, note: '', source: 'page', outside: false, created_by: VERA, created_at: '2026-10-14T08:00:00Z' });
function page(id: string, entries: Entry[] = [], revision = 7): PermissionPage {
  return { object: { kind: 'topic', id, name: id }, page: 'live', owner: VERA, owners: [], owners_writable: false, external: true, admins: { count: 1, of: ['system'] }, rows: [{ permission: 'topic.view', profile: 'any', inherit: true, inherited: { holders: [], from: 'system' }, entries, locked: null, writable: true, can_give: true }], state: 'follows', followers: null, gives: { choices: ['full_view'], roles: [], agents: [], holders_share: true }, covers: null, depends: [], reaches: null, revision };
}
function preview(): SharePreview {
  return { allowed: true, revision: 7, permissions: ['topic.view'], holders: [{ kind: 'user', user: VERA }], effects: { pages: 2, orgs: null, gaining: [{ user: VERA, permissions: ['topic.view'], pages: 2 }], losing: [], more: 0 }, already: [{ user: VERA, permissions: ['doc.view'], pages: 1 }], already_more: 1, agents: [], rows: [{defaults:false,permission:'topic.view',profile:'any',before:[],after:[{...entry('trial-only'),options:{max_tokens:500},expires_at:'2099-01-01T00:00:00Z'}]}], rules: [{ key: 'sharing.allow_holders', value: true, source: 'system', sharing: true }, { key: 'capability.policy.port_forward', value: false, source: null, sharing: false }], refusal: null };
}
const targets = ['First', 'Refused', 'Lost'].map((id) => ({ kind: 'topic' as const, id, name: id }));
async function review() {
  await screen.findAllByText('They will be named here.');
  await userEvent.type(screen.getByRole('combobox', { name: 'Add people, groups or roles' }), 'Vera');
  await userEvent.click(await screen.findByRole('option', { name: 'Vera Rubin' }));
  await userEvent.click(screen.getByRole('button', { name: 'Review shares' }));
  await screen.findAllByRole('region', { name: 'Sharing consequences' });
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.read.mockImplementation((target) => Promise.resolve(page(target.id)));
  mocks.preview.mockResolvedValue(preview());
});

it('reviews policy, existing access and gains first; sends frozen revisions and offers Undo for the confirmed rows alone', async () => {
  const refusal = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Refused', detail: 'Sharing changed on this row.', request_id: '' }, null);
  mocks.change.mockResolvedValueOnce(page('First', [entry('new-entry')], 8)).mockRejectedValueOnce(refusal).mockRejectedValueOnce(new Error('The reply was lost.')).mockResolvedValue(page('First', [], 9));
  show(<ShareSeveralDialog targets={targets} open onClose={vi.fn()} />);
  await review();
  expect(mocks.change).not.toHaveBeenCalled();
  expect(screen.getAllByText('Already holds')).toHaveLength(3);
  expect(screen.getAllByText(/Applies to later use/)).toHaveLength(3);
  expect(screen.getAllByText(i18n.t('settingRegistry.sharing.allow_holders.title'))).toHaveLength(3);
  expect(screen.queryByText('sharing.allow_holders', { exact: true })).not.toBeInTheDocument();
  const bounds = screen.getAllByText('max tokens', { selector: 'dt', exact: true });
  expect(bounds).toHaveLength(3);
  for (const bound of bounds) {
    const row = bound.closest('div');
    if (row === null) throw new Error('The proposed entry bound has no value row');
    expect(within(row).getByText('500', { exact: true })).toBeVisible();
  }
  await userEvent.click(screen.getByRole('button', { name: 'Apply to 3 ready rows' }));
  expect(await screen.findByText('Answer lost; outcome unknown')).toBeVisible();
  expect(mocks.change.mock.calls.slice(0, 3).map(([target, body]) => [target.id, body])).toEqual(targets.map((target) => [target.id, { expected_revision: 7, name: [{ choice: 'full_view', holders: [{ kind: 'user', id: 'vera' }] }] }]));
  const lost = within(screen.getByText('Lost', { selector: 'span' }).closest('li')!);
  mocks.read.mockResolvedValueOnce(page('Lost', [entry('unknown-writer')], 9));
  await userEvent.click(lost.getByRole('button', { name: 'Read current state' }));
  expect(await lost.findByText(/revision 9/)).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Undo successful rows' }));
  await waitFor(() => expect(mocks.change).toHaveBeenCalledTimes(4));
  expect(mocks.change.mock.calls[3]).toEqual([targets[0], { expected_revision: 8, rows: [{ permission: 'topic.view', remove: ['new-entry'], update: [] }] }]);
});

it('keeps a sharing refusal and its governing rule visible without applying it', async () => {
  mocks.preview.mockResolvedValue({ ...preview(), allowed: false, refusal: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Sharing kept', detail: 'Only owners may share this place.', request_id: '' } });
  show(<ShareSeveralDialog targets={[targets[0]!]} open onClose={vi.fn()} />);
  await review();
  expect(screen.getByRole('button', { name: 'Apply to 0 ready rows' })).toBeDisabled();
  const rule = screen.getByText(i18n.t('settingRegistry.sharing.allow_holders.title')).closest('li');
  if (rule === null) throw new Error('The governing sharing rule is missing');
  expect(within(rule).getByText(i18n.t('common.on'), { exact: true })).toBeVisible();
  expect(within(rule).getByText(i18n.t('shareReview.system'), { exact: true })).toBeVisible();
  expect(screen.queryByText('sharing.allow_holders', { exact: true })).not.toBeInTheDocument();
  expect(mocks.change).not.toHaveBeenCalled();
});

it('Undo restores an existing entry’s old expiry/options and never resets other holders or inheritance', () => {
  const old = { ...entry('old'), expires_at: '2026-12-01T00:00:00Z', options: { limit: 1 }, note: 'Keep me' };
  const before = page('First', [old]);
  const after = page('First', [{ ...old, revision: 2, expires_at: null, options: { limit: 2 }, note: 'Changed' }, entry('new')], 8);
  expect(inverseShare(before, after)).toEqual({ expected_revision: 8, rows: [{ permission: 'topic.view', remove: ['new'], update: [{ entry_id: 'old', options: { limit: 1 }, expires_at: old.expires_at, note: 'Keep me' }] }] });
});
