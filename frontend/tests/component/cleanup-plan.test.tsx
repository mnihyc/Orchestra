/** Cleanup requires an explicit data fate and name, keeps blockers factual and preserves partial results. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Cleanup } from '@/features/storage/Cleanup';
import type { CleanupPlan, CleanupRequest, CleanupStart, Job } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { storageReport } from '../doubles/storage-report';
import { show } from './support';

const calls = vi.hoisted(() => ({
  plan: vi.fn<(request: CleanupRequest) => Promise<CleanupPlan>>(),
  start: vi.fn<(body: CleanupStart) => Promise<Job>>(),
  read: vi.fn<(id: string) => Promise<Job>>(),
  cancel: vi.fn<(id: string) => Promise<void>>(),
}));
vi.mock('@/api/storage', () => ({ planCleanup: calls.plan, startCleanup: calls.start }));
vi.mock('@/api/exports', async (original) => ({ ...await original<object>(), readJob: calls.read, cancelJob: calls.cancel }));
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
});
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

it('shows an away item as blocked and keeps its data without adding an impossible fate', async () => {
  show(<Cleanup request={request} onClose={vi.fn()} />);
  const table = await screen.findByRole('table', { name: 'Cleanup plan' });
  const row = within(table).getByText('Connector version').closest('tr');
  if (row === null) throw new Error('Every cleanup item is a table row.');
  expect(within(row).getByText('Node away')).toBeVisible();
  expect(within(row).queryByRole('combobox', { name: 'Fate of Connector version' })).not.toBeInTheDocument();
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
  expect(within(results).getByText('Cached image').closest('tr')).toHaveTextContent('Removed');
  expect(within(results).getByText('Workspace recovery').closest('tr')).toHaveTextContent('Kept');
  expect(within(results).getByText('Connector version').closest('tr')).toHaveTextContent('Blocked');
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
