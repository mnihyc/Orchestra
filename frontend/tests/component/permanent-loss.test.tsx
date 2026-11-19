/** Permanent loss requires exact consent, and independent recovery and batch destinations retain their reviewed choices. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DeclareLostDialog } from '@/features/resources/DeclareLost';
import { MoveDialog } from '@/features/fleet/MoveDialog';
import { BatchChange } from '@/features/change/BatchChange';
import { EnvironmentStartRecovery } from '@/features/thread/EnvironmentStartRecovery';
import type { EnvironmentBatch, EnvironmentRecoveryPlan, EnvironmentRecoverPreflightBody, LossPlan, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { show } from './support';

const calls = vi.hoisted(() => ({ declare: vi.fn(), recovery: vi.fn<(body: EnvironmentRecoverPreflightBody) => Promise<EnvironmentRecoveryPlan>>(), start: vi.fn(), batch: vi.fn(), startEnvironment: vi.fn(), change: vi.fn(), count: vi.fn(), help: vi.fn() }));
const loss = { admin_grants: [], notification: { people: 0, default_on: false }, provider_threads: [], provider_threads_hidden: 0, preflight_id: 'loss-plan', name: 'host-2', environments: [{ environment: { id: 'env', name: 'Main' }, threads: [{ id: 'thread', name: 'Visible work' }], threads_hidden: 2, kept_bytes: 4096 }], providers: [], mounts: [], jobs: 1, kept_bytes: 4096 } as unknown as LossPlan;
const environment = { id: 'destination', name: 'Available', address: 'team/Available', status: 'running', access: { permissions: ['environment.manage'] } };
const query = (key: string, value: unknown) => ({ queryKey: [key], queryFn: () => Promise.resolve(value) });
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.help }));
vi.mock('@/api/loss', () => ({ lossPlanQuery: () => query('loss-plan', loss), declareLost: calls.declare, recoverableQuery: () => query('recoverable', { environments: [], others: 0 }), recoveryBatchQuery: () => query('recovery-batch', null), planRecovery: calls.recovery, startRecovery: calls.start }));
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), environmentsQuery: () => query('environments', [environment]), startEnvironment: calls.startEnvironment }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), hostsQuery: () => query('hosts', []), providersQuery: () => query('providers', []), agentsQuery: () => query('agents', []) }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), planBatch: calls.batch, jobQuery: (id: string) => ({ queryKey: ['recovery-count', id], queryFn: calls.count }) }));
vi.mock('@/features/change/target', async (original) => ({ ...await original<object>(), openChange: calls.change }));

beforeEach(() => {
  vi.clearAllMocks();
  calls.declare.mockResolvedValue(loss);
  calls.batch.mockResolvedValue({ preflight_id: 'batch-plan', executable: 0, plans: [], skipped: [], turns_running: 0, options: { wait_minutes: 10 }, estimate: { seconds_low: 1, seconds_high: 2 } });
});
afterEach(async () => { await setLocale('en'); });

it('keeps the destructive confirmation and its name disabled while permanent loss is being committed',async()=>{
  calls.declare.mockImplementation(()=>new Promise<LossPlan>(()=>undefined));
  const user=userEvent.setup();show(<DeclareLostDialog target={{type:'host',id:'host'}} name="host-2" onClose={vi.fn()}/>);
  const typed=await screen.findByRole('textbox',{name:i18n.t('loss.typeName',{name:'host-2'})});
  await user.click(typed);await user.paste('host-2');
  const confirm=screen.getByRole('button',{name:i18n.t('loss.declare')});await user.click(confirm);
  await waitFor(()=>expect(calls.declare).toHaveBeenCalledOnce());
  expect(confirm).toBeDisabled();expect(typed).toBeDisabled();
  await user.dblClick(confirm);expect(calls.declare).toHaveBeenCalledOnce();
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`requires the exact name and exposes only visible thread names (${locale})`, async () => {
    await setLocale(locale);
    const closed = vi.fn();
    show(<DeclareLostDialog target={{ type: 'host', id: 'host' }} name="host-2" onClose={closed} />);
    const user = userEvent.setup();
    const typed = await screen.findByRole('textbox', { name: i18n.t('loss.typeName', { name: 'host-2' }) });
    const hint = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('help.hints.loss-declare.title') }) });
    await waitFor(() => expect(hint).toHaveAttribute('data-help-heading', 'declare-lost'));
    await user.click(hint);
    await user.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
    expect(calls.help).toHaveBeenCalledWith('recover-lost-work', 'declare-lost');
    const confirm = screen.getByRole('button', { name: i18n.t('loss.declare') });
    expect(confirm).toBeDisabled();
    await user.type(typed, 'HOST-2');
    expect(confirm).toBeDisabled();
    expect(calls.declare).not.toHaveBeenCalled();
    await user.clear(typed); await user.type(typed, 'host-2');
    expect(confirm).toBeEnabled();
    await user.click(confirm);
    await waitFor(() => expect(calls.declare).toHaveBeenCalledOnce());
    expect(calls.declare.mock.calls[0]?.[0]).toEqual({ preflight_id: 'loss-plan', confirm: 'host-2', notify: false });
    expect(await screen.findByText(i18n.t('hostCustody.declared', { name: 'host-2' }))).toBeVisible();
    expect(screen.getByText('Visible work')).toBeInTheDocument();
    expect(closed).not.toHaveBeenCalled();
    const footer=screen.getByRole('dialog').querySelector('footer');
    expect(footer).not.toBeNull();
    await user.click(within(footer!).getByRole('button', { name: i18n.t('common.close') }));
    await waitFor(() => expect(closed).toHaveBeenCalledOnce());
  });

  it(`offers an alternative for a lost environment without retrying the revoked machine (${locale})`, async () => {
    await setLocale(locale);
    const thread = { id: 'thread', name: 'Work', environment: { id: 'env', name: 'Main', status: 'lost' }, environment_permissions: ['environment.manage'], access: { permissions: ['thread.manage'] }, owner: { display_name: 'Owner' }, pending_inputs: 1, blocked: { reason: 'environment_unavailable' } } as unknown as Thread;
    show(<EnvironmentStartRecovery thread={thread} />);
    expect(screen.getByText(i18n.t('loss.blocked'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('startRecovery.retry') })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('loss.elsewhere') }));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'thread', name: 'Work' }, { kind: 'switch', focus: 'environment' });
    expect(calls.startEnvironment).not.toHaveBeenCalled();
  });
}

it('names an existing destination in the batch request even when each thread keeps its provider', async () => {
  show(<BatchChange source={{ kind: 'threads', threads: [{ id: 'first', name: 'First' }, { id: 'second', name: 'Second' }], label: 'Selected' }} batch={null} open onClose={vi.fn()} />);
  const user = userEvent.setup();
  expect(await screen.findByRole('combobox', { name: i18n.t('loss.destination') })).toHaveTextContent(i18n.t('loss.keepEnvironment'));
  await user.click(screen.getByRole('combobox', { name: i18n.t('loss.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Available' }));
  await waitFor(() => expect(calls.batch).toHaveBeenCalled());
  expect(calls.batch.mock.calls.at(-1)?.[0]).toEqual({ thread_ids: ['first', 'second'], environment: { use: 'destination' } });
});

it.each(['en', 'zh-CN'] as const)('keeps a blocked environment independent and confirms the reviewed executable members once (%s)', async (locale) => {
  await setLocale(locale);
  const plan = { notification: { people: 0, default_on: false }, preflight_id: 'recover-plan', executable: 1, rows: [
    { environment: { id: 'ready', name: 'Ready', status: 'lost' }, action: 'recreate', host: null, files: { file_count: 4, missing: { count: 1 } }, listing_available: true, restored: false, restored_bytes: null, move_plan: null, switch_plan: null, blockers: [] },
    { environment: { id: 'blocked', name: 'Blocked', status: 'lost' }, action: 'recreate', host: null, files: null, listing_available: false, restored: false, restored_bytes: null, move_plan: null, switch_plan: null, blockers: [{ code: 'no_host', message: 'No compatible host' }] },
  ] } as unknown as EnvironmentRecoveryPlan;
  calls.recovery.mockResolvedValue(plan);
  calls.start.mockImplementation(() => new Promise<EnvironmentBatch>(() => undefined));
  show(<MoveDialog source={{ ids: ['ready', 'blocked'] }} open onClose={vi.fn()} />);
  expect(await screen.findByText(i18n.t('change.blocker.noHost'))).toBeVisible();
  expect(screen.getByRole('combobox', { name: i18n.t('loss.destinationHost') })).toHaveTextContent(i18n.t('changeCompact.automatic'));
  expect(screen.getByRole('combobox', { name: i18n.t('loss.hostFor', { name: 'Ready' }) })).toHaveTextContent(i18n.t('loss.asAbove'));
  const confirm = screen.getByRole('button', { name: i18n.t('loss.confirm') });
  expect(confirm).toBeEnabled();
  await userEvent.click(confirm);
  await waitFor(() => expect(calls.start).toHaveBeenCalledOnce());
  expect(calls.start.mock.calls[0]?.[0]).toEqual({ preflight_id: 'recover-plan', notify: false });
  expect(confirm).toBeDisabled();
  expect(calls.recovery.mock.calls[0]?.[0].environment_ids).toEqual(['ready', 'blocked']);
});

it('keeps missing workspace files visible when only some volumes were restored', async () => {
  const plan = { notification: { people: 0, default_on: false }, preflight_id: 'partly-restored', executable: 1, others: 0, rows: [{ environment: { id: 'home-only', name: 'Home restored', status: 'lost' }, action: 'recreate', host: null, files: { file_count: 4, bytes_to_download: 2, missing: { count: 1 } }, listing_available: true, restored: true, restored_bytes: 64, move_plan: null, switch_plan: null, blockers: [] }] } as unknown as EnvironmentRecoveryPlan;
  calls.recovery.mockResolvedValue(plan);
  show(<MoveDialog source={{ ids: ['home-only'] }} open onClose={vi.fn()} />);
  const restored = i18n.t('loss.restored', { size: formatBytes(64, 'en') });
  const coverage = i18n.t('loss.fileCoverage', { copied: '3', total: '4', missing: '1' });
  expect(await screen.findByText(`${restored} · ${coverage}`)).toBeVisible();
});

it('replans an optional unsupported home through the shared count subscription before permitting recovery', async () => {
  let finish: ((value: unknown) => void) | undefined;
  calls.count.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const ready = { notification: { people: 0, default_on: false }, preflight_id: 'counted', executable: 1, others: 0, admin_grants: [], rows: [{ environment: { id: 'moving', name: 'Move with record', status: 'running' }, action: 'move', host: null, files: null, listing_available: true, restored: false, restored_bytes: null, move_plan: { files: { file_count: 4, missing: { count: 0 }, home: null } }, switch_plan: null, blockers: [] }] } as unknown as EnvironmentRecoveryPlan;
  const counting = structuredClone(ready);
  counting.executable = 0;
  counting.preflight_id = 'counting';
  counting.rows[0]!.blockers = [{ code: 'measurement_pending', message: 'Waiting for the count' }];
  counting.rows[0]!.move_plan!.files.home = { measured: false, file_count: 0, total_bytes: 0, measurement: { job_id: 'home-count', measured_at: null } };
  calls.recovery.mockResolvedValueOnce(counting).mockResolvedValue(ready);
  calls.start.mockImplementation(() => new Promise<EnvironmentBatch>(() => undefined));
  show(<MoveDialog source={{ ids: ['moving'] }} open onClose={vi.fn()} />);
  await waitFor(() => expect(calls.count).toHaveBeenCalledOnce());
  const confirm = screen.getByRole('button', { name: i18n.t('loss.confirm') });
  expect(confirm).toBeDisabled();
  expect(calls.start).not.toHaveBeenCalled();
  finish?.({ state: 'failed', error: { code: 'unsupported' } });
  await waitFor(() => expect(calls.recovery).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(confirm).toBeEnabled());
  await userEvent.click(confirm);
  expect(calls.start.mock.calls[0]?.[0]).toEqual({ preflight_id: 'counted', notify: false });
});
