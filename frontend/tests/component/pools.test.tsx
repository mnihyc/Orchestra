/** A pool's written members can be inspected without disclosing or replacing unread members. */
import { queryOptions } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { PoolMembers } from '@/features/resources/pools/Members';
import type { Pool, PoolMembersQuery, PoolMembersReply } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ page: vi.fn<(after: PoolMembersQuery) => PoolMembersReply>(), put: vi.fn() }));
vi.mock('@/api/pools', () => ({ poolMembersQuery: (id: string, after: PoolMembersQuery) => queryOptions({ queryKey: ['written-members', id, after], queryFn: () => calls.page(after) }), putPoolMember: calls.put, putPoolMemberMetadata: calls.put, removePoolMember: calls.put, readPoolAfterWrite: vi.fn(() => Promise.resolve()) }));
const pool = { id: 'p', name: 'Build pool', kind: 'host', enabled: true, policy: { type: 'ordered' }, revision: 2,
  access: { administers: false, through: [], permissions: ['pool.view', 'pool.manage'] } } as Pool;

it('keeps a hidden written member hidden and reads the next page only when asked', async () => {
  const next: PoolMembersQuery = { after_id: 'hidden-row', after_position: 0 };
  calls.page.mockImplementation((after) => after.after_id === null ? {
    items: [{ id: 'hidden-row', position: 0, enabled: true, priority: null, weight: null, rule: null, label: null, reason: 'member_hidden' }], next,
  } : { items: [{ id: 'last-row', position: 1, enabled: true, priority: null, weight: null,
    rule: { type: 'single', resource: { kind: 'host', id: 'visible-host' } }, label: 'Build host', reason: null }], next: null });
  show(<PoolMembers pool={pool} />);
  expect(await screen.findByText(i18n.t('pools.hidden'))).toBeInTheDocument();
  expect(screen.queryByText('hidden-row')).not.toBeInTheDocument();
  expect(calls.page).toHaveBeenCalledTimes(1);
  expect(calls.put).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('documentFolders.next') }));
  expect(await screen.findByText('Build host')).toBeInTheDocument();
  expect(calls.page).toHaveBeenLastCalledWith(next);
  expect(calls.put).not.toHaveBeenCalled();
});
