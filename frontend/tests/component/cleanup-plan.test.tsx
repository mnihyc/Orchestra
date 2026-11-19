/** Cleanup requires an explicit data fate and name, keeps blockers factual and preserves partial results. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { Cleanup } from '@/features/storage/Cleanup';
import type { CleanupPlan, CleanupRequest, CleanupStart, Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { storageReport } from '../doubles/storage-report';
import { show } from './support';

const calls = vi.hoisted(() => ({
  plan: vi.fn<(request: CleanupRequest) => Promise<CleanupPlan>>(),
  start: vi.fn<(body: CleanupStart) => Promise<Job>>(),
  read: vi.fn<(id: string) => Promise<Job>>(),
  cancel: vi.fn<(id: string) => Promise<void>>(),
  deletePlan: vi.fn<(id: string) => Promise<CleanupPlan>>(),
  deleteMount: vi.fn<(id: string, body: CleanupStart) => Promise<Job>>(),
}));
vi.mock('@/api/storage', () => ({ planCleanup: calls.plan, startCleanup: calls.start }));
vi.mock('@/api/exports', async (original) => ({ ...await original<object>(), readJob: calls.read, cancelJob: calls.cancel }));
vi.mock('@/api/resource-tools', async (original) => ({ ...await original<object>(), planMountDeletion: calls.deletePlan, deleteMount: calls.deleteMount }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const request: CleanupRequest = { scope: 'me', resource: null, items: ['image', 'recovery', 'tool'] };
const now = '2026-10-01T00:00:00Z';
function plan(): CleanupPlan {
  const [image, recovery, away] = storageReport().items;
  if (image === undefined || recovery === undefined || away === undefined) throw new Error('The fixture has all three storage states.');
  return { id: 'cleanup-plan', expires_at: now, confirmation_name: 'Workspace', people: [], other_people: 0, notify: false, items: [
    { record: image, choices: ['keep', 'remove'], selected: 'remove' },
    { record: recovery, choices: ['keep', 'purge'], selected: 'keep' },
    { record: away, choices: ['keep'], selected: 'keep' },
  ] };
}
function job(state: Job['state'] = 'running'): Job {
  return { id: 'cleanup-job', kind: 'storage_cleanup', state, node_id: null, environment_id: null, provider_id: null, batch_id: null,
    created_at: now, created_by: null, started_at: now, finished_at: state === 'running' ? null : now, error: null, log: null,
    progress: { phase: 'remove', percent: state === 'running' ? 50 : 100, phases: [], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, eta_seconds: null, rate_bps: null, message: null },
    result: { cleanup: [{ id: 'image', outcome: 'removed', reason: null }, { id: 'recovery', outcome: 'kept', reason: null }, { id: 'tool', outcome: 'blocked', reason: 'node_away' }] },
  };
}
beforeEach(() => {
  calls.plan.mockResolvedValue(plan());
  calls.start.mockResolvedValue(job());
  calls.read.mockResolvedValue(job());
  calls.cancel.mockResolvedValue();
  calls.deletePlan.mockResolvedValue({ ...plan(), items: [] });
  calls.deleteMount.mockResolvedValue(job());
});
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`deletes a mount through its own operation while keeping every data item without typing a name in ${locale}`, async () => {
    await setLocale(locale);
    const preview = plan();
    preview.items = preview.items.map((item) => ({ ...item, choices: ['keep'], selected: 'keep' }));
    calls.deletePlan.mockResolvedValue(preview);
    const user = userEvent.setup();
    show(<Cleanup intent="delete-mount" request={{ scope: 'me', resource: { kind: 'mount', id: 'mount' }, items: [] }} onClose={vi.fn()} />);
    await screen.findByRole('table', { name: i18n.t('storage.cleanup.plan') });
    const submit = screen.getByRole('button', { name: i18n.t('resourceDetails.remove') });
    expect(submit).toBeEnabled();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    await user.click(submit);
    await waitFor(() => expect(calls.deleteMount).toHaveBeenCalledExactlyOnceWith('mount', {
      plan_id: preview.id, choices: preview.items.map((item) => ({ id: item.record.id, choice: 'keep' })), confirmation_name: null, notify: false,
    }));
    expect(calls.start).not.toHaveBeenCalled();
    expect(calls.plan).not.toHaveBeenCalled();
  });

  it(`allows deletion of an empty mount through the same plan and retained job in ${locale}`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<Cleanup intent="delete-mount" request={{ scope: 'me', resource: { kind: 'mount', id: 'empty' }, items: [] }} onClose={vi.fn()} />);
    await screen.findByRole('table', { name: i18n.t('storage.cleanup.plan') });
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.remove') }));
    await waitFor(() => expect(calls.deleteMount).toHaveBeenCalledExactlyOnceWith('empty', expect.objectContaining({ choices: [], confirmation_name: null })));
    expect(await screen.findByRole('button', { name: i18n.t('storage.cleanup.cancelRemaining') })).toBeVisible();
  });
}

for(const locale of ['en','zh-CN'] as const) it(`keeps a generated volume handle behind Copy ID throughout cleanup without changing its removal target in ${locale}`,async()=>{
  await setLocale(locale);
  const preview=plan(),base=preview.items[0];
  if(base===undefined)throw new Error('The fixture has a retained source.');
  const record={...base.record,id:'native-volume',kind:'volume' as const,name:'orch-ws-00000000-0000-4000-8000-000000000001'};
  preview.items=[{...base,record}];
  calls.plan.mockResolvedValue(preview);
  calls.read.mockResolvedValue({...job('succeeded'),result:{cleanup:[{id:record.id,outcome:'removed',reason:null}]}});
  const user=userEvent.setup();
  show(<Cleanup request={{...request,items:[record.id]}} onClose={vi.fn()} />);
  const table=await screen.findByRole('table',{name:i18n.t('storage.cleanup.plan')});
  expect(table).toHaveTextContent(i18n.t('storage.kinds.volume'));
  expect(table.outerHTML).not.toContain(record.name);
  expect(within(table).getByRole('combobox',{name:i18n.t('storage.cleanup.fateFor',{name:i18n.t('storage.kinds.volume')})})).toBeVisible();
  await user.click(within(table).getByRole('button',{name:i18n.t('common.copyId')}));
  expect(await navigator.clipboard.readText()).toBe(record.name);
  await user.click(screen.getByRole('button',{name:i18n.t('storage.clean')}));
  await waitFor(()=>expect(calls.start).toHaveBeenCalledExactlyOnceWith({plan_id:preview.id,confirmation_name:null,notify:false,choices:[{id:record.id,choice:'remove'}]}));
  const results=await screen.findByRole('table',{name:i18n.t('storage.cleanup.results')});
  expect(results).toHaveTextContent(i18n.t('storage.kinds.volume'));
  expect(results.outerHTML).not.toContain(record.name);
});

it('shows an away item as blocked and keeps its data without adding an impossible fate', async () => {
  show(<Cleanup request={request} onClose={vi.fn()} />);
  const table = await screen.findByRole('table', { name: 'Cleanup plan' });
  const row = within(table).getByText('Tool version').closest('tr');
  if (row === null) throw new Error('Every cleanup item is a table row.');
  expect(within(row).getByText('Node away')).toBeVisible();
  expect(within(row).queryByRole('combobox', { name: 'Fate of Tool version' })).not.toBeInTheDocument();
  expect(within(row).getByText('Keep', { exact: true })).toBeVisible();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
});

it('requires the exact typed name after Purge now and sends every displayed fate once', async () => {
  const user = userEvent.setup();
  show(<Cleanup request={request} onClose={vi.fn()} />);
  await user.click(await screen.findByRole('combobox', { name: 'Fate of Workspace recovery' }));
  await user.click(screen.getByRole('option', { name: 'Purge now' }));
  const submit = screen.getByRole('button', { name: 'Clean up…' });
  const name = screen.getByRole('textbox', { name: 'Type Workspace to erase data' });
  expect(submit).toBeDisabled();
  await user.type(name, 'workspace');
  expect(submit).toBeDisabled();
  expect(calls.start).not.toHaveBeenCalled();
  await user.clear(name); await user.type(name, 'Workspace');
  await user.click(submit);
  await waitFor(() => expect(calls.start).toHaveBeenCalledExactlyOnceWith({ plan_id: 'cleanup-plan', confirmation_name: 'Workspace', notify: false, choices: [
    { id: 'image', choice: 'remove' }, { id: 'recovery', choice: 'purge' }, { id: 'tool', choice: 'keep' },
  ] }));
  expect(await screen.findByRole('button', { name: 'Cancel remaining' })).toBeVisible();
});

it('cancel waits for the server result and retains the completed item beside kept and blocked outcomes', async () => {
  const user = userEvent.setup();
  const close = vi.fn();
  show(<Cleanup request={request} onClose={close} />);
  await user.click(await screen.findByRole('button', { name: 'Clean up…' }));
  const cancel = await screen.findByRole('button', { name: 'Cancel remaining' });
  calls.read.mockResolvedValue(job('cancelled'));
  await user.click(cancel);
  await waitFor(() => expect(calls.cancel).toHaveBeenCalledExactlyOnceWith('cleanup-job'));
  const results = await screen.findByRole('table', { name: 'Cleanup results' });
  expect(within(results).getByText('Image').closest('tr')).toHaveTextContent('Removed');
  expect(within(results).getByText('Workspace recovery').closest('tr')).toHaveTextContent('Kept');
  expect(within(results).getByText('Tool version').closest('tr')).toHaveTextContent('Blocked');
  expect(screen.queryByRole('button', { name: 'Cancel remaining' })).not.toBeInTheDocument();
  expect(close).not.toHaveBeenCalled();
});

it('publishes the actual accepted job and its confirmed outcome once to the resource view',async()=>{
  const user=userEvent.setup();const accepted=vi.fn();const finished=vi.fn();
  calls.read.mockResolvedValue(job('succeeded'));
  show(<Cleanup request={request} onClose={vi.fn()} onAccepted={accepted} onFinished={finished} />);
  await user.click(await screen.findByRole('button',{name:'Clean up…'}));
  await waitFor(()=>expect(accepted).toHaveBeenCalledExactlyOnceWith(job()));
  await waitFor(()=>expect(finished).toHaveBeenCalledExactlyOnceWith(job('succeeded')));
  expect(await screen.findByRole('table',{name:'Cleanup results'})).toBeVisible();
});

it('operator-owned files offer their exact copyable command without an API erase control', async () => {
  const user=userEvent.setup(), preview=plan();
  const command="'/opt/orch/deploy/update.sh' prune";
  const base=preview.items[0];
  if(base===undefined)throw new Error('The fixture has a retained source.');
  preview.items=[{record:{...base.record,id:'release:server:old',kind:'release',name:'server/old',operator_command:command},choices:['keep'],selected:'keep'}];
  calls.plan.mockResolvedValue(preview);
  show(<Cleanup request={{scope:'system',resource:null,items:['release:server:old']}} onClose={vi.fn()} />);
  const copy=await screen.findByRole('button',{name:'Copy server command'});
  expect(screen.getByText(command,{exact:true})).toBeVisible();
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Clean up…'})).not.toBeInTheDocument();
  const written=vi.spyOn(navigator.clipboard,'writeText').mockResolvedValue();
  await user.click(copy);
  await waitFor(()=>expect(written).toHaveBeenCalledExactlyOnceWith(command));
  expect(calls.start).not.toHaveBeenCalled();
  written.mockRestore();
});

function planWithPlaces(): CleanupPlan {
  return { ...plan(), items: [], other_people: 1, effects: {
    subject: { kind: 'resource', object: { kind: 'mount', id: 'mount' } }, as_of: now, uses: null, dependents_complete: true,
    dependents: [{ relation: 'explicit_giving', count: 2, hidden: 1, until: null,
      visible: [{ name: 'Workbench', object: { kind: 'topic', id: 'topic' }, relation: 'explicit_giving', until: null }] }],
  } };
}

it('lists visible and hidden mount uses before switching them off and deleting in one act', async () => {
  const preview = planWithPlaces();
  calls.deletePlan.mockResolvedValue(preview);
  show(<Cleanup intent="delete-mount" request={{scope:'me',resource:{kind:'mount',id:'mount'},items:[]}} onClose={vi.fn()} />);
  expect(await screen.findByText('Workbench')).toBeVisible();
  expect(screen.getByText(i18n.t('storage.hidden', { number: '1' }))).toBeVisible();
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  await userEvent.click(screen.getByRole('button', { name: 'Switch off and delete' }));
  await waitFor(() => expect(calls.deleteMount).toHaveBeenCalledExactlyOnceWith('mount', { plan_id:preview.id, choices:[], confirmation_name:null, notify:false }));
});

it('names the place that prevents switching off a mount and keeps the delete action disabled', async () => {
  const preview = planWithPlaces();
  preview.mount_switch_off_blocked = { relation:'explicit_giving',count:1,hidden:0,until:null,
    visible:[{name:'Workbench',object:{kind:'topic',id:'topic'},relation:'explicit_giving',until:null}] };
  calls.deletePlan.mockResolvedValue(preview);
  show(<Cleanup intent="delete-mount" request={{scope:'me',resource:{kind:'mount',id:'mount'},items:[]}} onClose={vi.fn()}/>);
  expect(await screen.findByText('You cannot switch off this mount in Workbench')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Switch off and delete' })).toBeDisabled();
  expect(calls.deleteMount).not.toHaveBeenCalled();
});

function refuseMountConfirmation() {
  calls.deleteMount.mockRejectedValueOnce(new ApiError({
    type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Access denied',
    detail: 'Access changed', status: 403, request_id: 'mount-confirmation',
  }, null));
}

it('reads the changed place permissions after a refused confirmation and names the blocked topic', async () => {
  const latest = planWithPlaces();
  latest.mount_switch_off_blocked = {
    relation: 'explicit_giving', count: 1, hidden: 0, until: null,
    visible: [{ name: 'Workbench', object: { kind: 'topic', id: 'topic' }, relation: 'explicit_giving', until: null }],
  };
  calls.deletePlan.mockResolvedValueOnce(planWithPlaces()).mockResolvedValue(latest);
  refuseMountConfirmation();
  show(<Cleanup intent="delete-mount" request={{ scope: 'me', resource: { kind: 'mount', id: 'mount' }, items: [] }} onClose={vi.fn()} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Switch off and delete' }));
  expect(await screen.findByText('You cannot switch off this mount in Workbench')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Switch off and delete' })).toBeDisabled();
  expect(calls.deleteMount).toHaveBeenCalledOnce();
  expect(calls.deletePlan).toHaveBeenCalledTimes(2);
});

it('keeps a refused deletion unavailable until the changed places can be read again', async () => {
  calls.deletePlan.mockResolvedValueOnce(planWithPlaces()).mockRejectedValueOnce(new TypeError('Failed to fetch'));
  refuseMountConfirmation();
  show(<Cleanup intent="delete-mount" request={{ scope: 'me', resource: { kind: 'mount', id: 'mount' }, items: [] }} onClose={vi.fn()} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Switch off and delete' }));
  const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.getByRole('button', { name: 'Switch off and delete' })).toBeDisabled();
  calls.deletePlan.mockResolvedValue(planWithPlaces());
  await userEvent.click(retry);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Switch off and delete' })).toBeEnabled());
  expect(calls.deleteMount).toHaveBeenCalledOnce();
});
