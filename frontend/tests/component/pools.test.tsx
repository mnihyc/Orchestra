/** A pool's written members can be inspected without disclosing or replacing unread members. */
import { queryOptions } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type * as PoolsApi from '@/api/pools';
import type * as ResourceLibraryApi from '@/api/resourceLibrary';
import { PoolMembers } from '@/features/resources/pools/Members';
import { PoolRecover } from '@/features/resources/pools/Recover';
import { PoolDialog } from '@/features/resources/pools/PoolDialog';
import { DeletePool } from '@/features/resources/pools/Delete';
import type { LibraryEntry, Pool, PoolMembersQuery, PoolMembersReply } from '@/generated/api-types';
import { api } from '@/api/client';
import { i18n } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ page: vi.fn<(after: PoolMembersQuery) => PoolMembersReply>(), put: vi.fn(), recover: vi.fn(), duplicate: vi.fn(), readback: vi.fn(), remove: vi.fn(), library: vi.fn<() => LibraryEntry[]>() }));
vi.mock('@/api/pools', () => ({ poolMembersQuery: (id: string, after: PoolMembersQuery) => queryOptions({ queryKey: ['written-members', id, after], queryFn: () => calls.page(after) }), createPool:calls.put,updatePool:calls.put,duplicatePool:calls.duplicate,putPoolMember: calls.put, putPoolMemberMetadata: calls.put, removePoolMember: calls.put, recoverPool: calls.recover, readPoolAfterWrite: calls.readback, deletePool: calls.remove }));
vi.mock('@/api/resourceLibrary', async (original) => ({
  ...await original<typeof ResourceLibraryApi>(),
  libraryQuery: () => queryOptions({queryKey:['delete-pool-references'],queryFn:()=>calls.library()}),
}));
vi.mock('@/features/resources/pools/ResourceChoice', () => ({
  PoolResourceChoice: ({ onChange }: { onChange: (id: string) => void }) =>
    <button type="button" onClick={() => onChange('provider')}>Example provider</button>,
}));
const pool: Pool = { address: '', description: '', lend_members: false, owner: null, home: null, settings: [], grouping: {home:null,home_path:[],origin_machine:null,availability:[]}, target: {type:'pool',id:'p',kind:'host'}, id: 'p', name: 'Build pool', kind: 'host', enabled: true, policy: { type: 'ordered' }, revision: 2,
  access: { administers: false, through: [], permissions: ['pool.view', 'pool.manage'] } };

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

it('closes Add member on its saved receipt without a second write or a blocking metadata read', async () => {
  const { putPoolMember } = await vi.importActual<typeof PoolsApi>('@/api/pools');
  const write = vi.spyOn(api, 'PUT').mockResolvedValue({
    data: { projection: 'receipt', id: pool.id, revision: 3 },
    response: new Response(null, { status: 200 }),
  });
  calls.put.mockImplementationOnce(putPoolMember);
  calls.page.mockReturnValue({ items: [], next: null });
  const before = calls.readback.mock.calls.length;
  try {
    show(<PoolMembers pool={{ ...pool, kind: 'model' }} />);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('pools.addMember') }));
    await userEvent.click(screen.getByRole('button', { name: 'Example provider' }));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(write).toHaveBeenCalledTimes(1);
    expect(calls.readback).toHaveBeenCalledTimes(before);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  } finally { write.mockRestore(); }
});

it('reads an accepted pool recovery again without sending a second recovery action', async () => {
  calls.recover.mockResolvedValue({projection:'receipt',id:pool.id,revision:3});
  calls.readback.mockRejectedValueOnce(new Error('Read acknowledgement interrupted')).mockResolvedValueOnce(pool);
  show(<PoolRecover pool={pool} />);
  await userEvent.click(screen.getByRole('button', {name:i18n.t('pools.recover')}));
  expect(await screen.findByRole('alert')).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', {name:i18n.t('pools.readAgain')}));
  expect(await screen.findByRole('button', {name:i18n.t('pools.recover')})).toBeInTheDocument();
  expect(calls.recover).toHaveBeenCalledTimes(1);
  expect(calls.readback).toHaveBeenCalledTimes(2);
});


it('shows the ordinary protected reference list before a pool delete is confirmed', async () => {
  calls.library.mockReturnValue([{target:{object:{kind:'pool',id:pool.id},version_id:null},threads:null,
    facts:{subject:{kind:'resource',object:{kind:'pool',id:pool.id}},as_of:'2026-11-01T00:00:00Z',dependents_complete:true,uses:null,
      dependents:[{relation:'explicit_giving',count:2,hidden:1,until:null,visible:[
        {object:null,name:'Visible settings place',relation:'explicit_giving',until:null},
      ]}]}}]);
  show(<DeletePool pool={pool} onClose={vi.fn()} onDeleted={vi.fn()} />);
  await userEvent.click(await screen.findByRole('button',{name:'Given 2 places'}));
  expect(await screen.findByText('Visible settings place')).toBeVisible();
  expect(screen.getByText('and 1 more')).toBeVisible();
  expect(calls.remove).not.toHaveBeenCalled();
});


it('leaves a deleted pool on the first confirmed receipt', async () => {
  const { deletePool } = await vi.importActual<typeof PoolsApi>('@/api/pools');
  const request = vi.spyOn(api, 'DELETE').mockResolvedValue({
    data: { projection: 'receipt', id: pool.id, revision: pool.revision + 1 },
    response: new Response(null, { status: 200 }),
  });
  calls.remove.mockImplementationOnce(deletePool);
  calls.library.mockReturnValue([]);
  const onDeleted = vi.fn();
  try {
    show(<DeletePool pool={pool} onClose={vi.fn()} onDeleted={onDeleted} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('pools.deleteName', { name: pool.name }) }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledTimes(1));
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith('/pools/{id}', expect.objectContaining({
      params: { path: { id: pool.id } },
      body: { expected_revision: pool.revision },
    }));
    expect(request.mock.calls).toHaveProperty([0, 1, 'headers', 'Idempotency-Key'], expect.any(String));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  } finally {
    request.mockRestore();
  }
});

it('reads an accepted duplicate again without copying hidden or unloaded rules in the browser', async () => {
  calls.duplicate.mockReset();
  calls.readback.mockReset();
  calls.duplicate.mockResolvedValue({projection:'receipt',id:'copied-pool',revision:1});
  calls.readback.mockRejectedValueOnce(new Error('Read acknowledgement interrupted')).mockResolvedValueOnce({...pool,id:'copied-pool'});
  const close=vi.fn();
  show(<PoolDialog copy={pool} initial={{kind:pool.kind,home:{id:'home',name:'Target home'}}} onClose={close} />);
  expect(screen.queryByLabelText(i18n.t('pools.policy.label'))).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:i18n.t('duplicate.now')}));
  const readAgain=await screen.findByRole('button',{name:i18n.t('pools.readAgain')});
  await waitFor(()=>expect(readAgain).toBeEnabled());
  await userEvent.click(readAgain);
  await waitFor(()=>expect(close).toHaveBeenCalledTimes(1));
  expect(calls.duplicate).toHaveBeenCalledTimes(1);
  expect(calls.duplicate.mock.calls[0]?.[1]).toEqual({expected_revision:pool.revision,home_id:'home',name:pool.name,access:'only_me'});
  expect(calls.readback).toHaveBeenCalledTimes(2);
});

it('closes New pool and opens the pool once the server accepts it, without an accepted-without-content notice', async () => {
  const { createPool } = await vi.importActual<typeof PoolsApi>('@/api/pools');
  const request = vi.spyOn(api, 'POST').mockResolvedValue({
    data: { projection: 'receipt', id: 'new-pool', revision: 1 },
    response: new Response(null, { status: 201 }),
  });
  calls.put.mockReset().mockImplementationOnce(createPool);
  calls.readback.mockReset().mockResolvedValue({ ...pool, id: 'new-pool' });
  const close = vi.fn(), created = vi.fn();
  try {
    show(<PoolDialog initial={{ kind: 'host', home: { id: 'home', name: 'Target home' } }} onClose={close} onCreated={created} />);
    await userEvent.type(screen.getByLabelText(i18n.t('pools.name')), 'Build pool');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('pools.create') }));
    await waitFor(() => expect(created).toHaveBeenCalledExactlyOnceWith('new-pool'));
    expect(close).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledTimes(1);
    expect(calls.readback).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(i18n.t('ui.receipt.accepted'))).not.toBeInTheDocument();
  } finally {
    request.mockRestore();
  }
});

it('takes every accepted pool change as its receipt rather than as a change whose content was not returned', async () => {
  const actual = await vi.importActual<typeof PoolsApi>('@/api/pools');
  const receipt = { projection: 'receipt' as const, id: pool.id, revision: pool.revision + 1 };
  const answer = { data: receipt, response: new Response(null, { status: 200 }) };
  const requests = [vi.spyOn(api, 'POST').mockResolvedValue(answer), vi.spyOn(api, 'PUT').mockResolvedValue(answer),
    vi.spyOn(api, 'PATCH').mockResolvedValue(answer), vi.spyOn(api, 'DELETE').mockResolvedValue(answer)];
  try {
    await expect(actual.createPool({ home_id: 'home', name: 'Build pool', description: '', access: 'only_me',
      definition: { kind: 'host', enabled: false, policy: null, lend_members: false, config: {}, members: [] } }, 'create-key')).resolves.toEqual(receipt);
    await expect(actual.duplicatePool(pool.id, { expected_revision: pool.revision, home_id: 'home', name: 'Copy', access: 'only_me' }, 'copy-key')).resolves.toEqual(receipt);
    await expect(actual.updatePool(pool.id, { expected_revision: pool.revision, name: pool.name, description: '', enabled: true, policy: null, lend_members: false, config: null })).resolves.toEqual(receipt);
    await expect(actual.putPoolMemberMetadata(pool.id, 'row', { expected_revision: pool.revision, enabled: true, priority: null, weight: null, position: 0 })).resolves.toEqual(receipt);
    await expect(actual.removePoolMember(pool.id, 'row', pool.revision)).resolves.toEqual(receipt);
    await expect(actual.recoverPool(pool.id, pool.revision, 'recover-key')).resolves.toEqual(receipt);
    await expect(actual.putPoolLending('entry', { scope: { kind: 'member', object: { kind: 'host', id: 'host' } }, expected_revision: null,
      matcher: { kind: 'pool', pool_id: pool.id }, options: {}, show_names: false, enabled: true, expires_at: null })).resolves.toEqual(receipt);
  } finally {
    for (const request of requests) request.mockRestore();
  }
});
