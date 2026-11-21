/** Permanent loss requires exact consent, and independent recovery and batch destinations retain their reviewed choices. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DeclareLostDialog } from '@/features/resources/DeclareLost';
import { MoveDialog } from '@/features/fleet/MoveDialog';
import { BatchChange } from '@/features/change/BatchChange';
import { EnvironmentStartRecovery } from '@/features/thread/EnvironmentStartRecovery';
import type { EnvironmentBatch, EnvironmentRecoveryPlan, EnvironmentRecoverPreflightBody, ForkBatch, LossPlan, Provider, ProviderModel, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { failedStart } from './failed-start-fixtures';
import { show, skipOverflowFallback } from './support';
import { detailAgent } from './resource-detail-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { resourcePlan } from './plan-resource-fixtures';

skipOverflowFallback();

const calls = vi.hoisted(() => ({ declare: vi.fn(), recovery: vi.fn<(body: EnvironmentRecoverPreflightBody) => Promise<EnvironmentRecoveryPlan>>(), start: vi.fn(), batch: vi.fn(), startEnvironment: vi.fn(), change: vi.fn(), count: vi.fn(), help: vi.fn() }));
const batchChoices = vi.hoisted(() => ({ providers: [] as Provider[], models: [] as ProviderModel[], running: null as ForkBatch | null }));
const loss = { admin_grants: [], notification: { people: 0, default_on: false }, provider_threads: [], provider_threads_hidden: 0, preflight_id: 'loss-plan', name: 'host-2', environments: [{ environment: { id: 'env', name: 'Main' }, threads: [{ id: 'thread', name: 'Visible work' }], threads_hidden: 2, kept_bytes: 4096 }], providers: [], mounts: [], jobs: 1, kept_bytes: 4096 } as unknown as LossPlan;
const environment = { id: 'destination', name: 'Available', address: 'team/Available', status: 'running', access: { permissions: ['environment.manage'] } };
const query = (key: string, value: unknown) => ({ queryKey: [key], queryFn: () => Promise.resolve(value) });
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => calls.help }));
vi.mock('@/api/loss', () => ({ lossPlanQuery: () => query('loss-plan', loss), declareLost: calls.declare, recoverableQuery: () => query('recoverable', { environments: [], others: 0 }), recoveryBatchQuery: () => query('recovery-batch', null), planRecovery: calls.recovery, startRecovery: calls.start }));
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), environmentsQuery: () => query('environments', [environment]), startEnvironment: calls.startEnvironment }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), hostsQuery: () => query('hosts', []), providersQuery: () => query('providers', batchChoices.providers), agentsQuery: () => query('agents', [detailAgent]), providerModelsQuery: () => query('batch-models', batchChoices.models) }));
vi.mock('@/api/resource-impact', async (original) => ({ ...await original<object>(), resourceImpactQuery: () => query('batch-impact', { name: 'Source', facts: { dependents: [{ relation: 'binding', count: 2, hidden: 0,
  visible: [{ object: { kind: 'thread', id: 'first' }, name: 'First' }, { object: { kind: 'thread', id: 'second' }, name: 'Second' }] }] } }) }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), planBatch: calls.batch, batchQuery: (id: string) => query(`batch-${id}`, batchChoices.running), jobQuery: (id: string) => ({ queryKey: ['recovery-count', id], queryFn: calls.count }) }));
vi.mock('@/features/change/target', async (original) => ({ ...await original<object>(), openChange: calls.change }));

beforeEach(() => {
  vi.clearAllMocks();
  batchChoices.providers = []; batchChoices.models = []; batchChoices.running = null;
  calls.declare.mockResolvedValue(loss);
  calls.batch.mockResolvedValue({ preflight_id: 'batch-plan', executable: 0, plans: [], skipped: [], turns_running: 0, options: { wait_minutes: 10 }, estimate: { seconds_low: 1, seconds_high: 2 } });
});
afterEach(async () => { await setLocale('en'); });

it('keeps every visible loss-plan resource destination in the current application document',async()=>{
  const view=show(<DeclareLostDialog target={{type:'host',id:'host'}} name="host-2" onClose={vi.fn()}/>);
  await screen.findByRole('textbox',{name:i18n.t('loss.typeName',{name:'host-2'})});
  for(const group of view.baseElement.querySelectorAll('details'))group.open=true;
  for(const link of screen.getAllByRole('link',{name:'Main'})){
    const event=new MouseEvent('click',{bubbles:true,cancelable:true,button:0});
    fireEvent(link,event);
    expect(event.defaultPrevented).toBe(true);
  }
  expect(calls.declare).not.toHaveBeenCalled();
});

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
    const { thread: native } = failedStart();
    if (native.environment === null) throw new Error('The lost environment keeps its native reference');
    const thread: Thread = { ...native, id: 'thread', name: 'Work',
      environment: { ...native.environment, id: 'env', status: 'lost' }, environment_permissions: ['environment.manage'],
      access: { ...native.access, permissions: ['thread.view'] }, can: { ...native.can, place: true },
    };
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
  expect(await screen.findByRole('combobox', { name: i18n.t('loss.destination') })).toHaveValue(i18n.t('loss.keepEnvironment'));
  await user.click(screen.getByRole('combobox', { name: i18n.t('loss.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Available' }));
  await waitFor(() => expect(calls.batch).toHaveBeenCalled());
  expect(calls.batch.mock.calls.at(-1)?.[0]).toEqual({ thread_ids: ['first', 'second'], environment: { use: 'destination' } });
});

it.each(['provider', 'environment'] as const)('keeps every chosen runtime field when an all-%s batch gains a destination or becomes a subset', async (kind) => {
  batchChoices.providers = [planProvider({ id: 'target-provider', name: 'Target', protocols: ['openai'] })];
  batchChoices.models = [{ ...MODEL_ESTIMATE, id: 'chosen-model', model_name: 'chosen-model', display_name: 'Chosen model', enabled: true, source: 'manual', groups: [],
    parameters: { efforts: ['low', 'high'], default_effort: 'low' } }];
  show(<BatchChange source={{ kind, id: 'source' }} batch={null} open onClose={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('combobox', { name: i18n.t('change.runs.provider') }));
  await user.click(await screen.findByRole('option', { name: 'Target' }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('change.runs.agent') }));
  await user.click(await screen.findByRole('option', { name: detailAgent.name }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('change.runs.model') }));
  await user.click(await screen.findByRole('option', { name: 'Chosen model' }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('change.runs.effort') }));
  await user.click(await screen.findByRole('option', { name: 'High' }));
  const target = { provider_id: 'target-provider', agent_id: detailAgent.id, model: 'chosen-model', effort: 'high' };
  await waitFor(() => expect(calls.batch.mock.calls.at(-1)?.[0]).toHaveProperty('to', target));
  await user.click(screen.getByRole('combobox', { name: i18n.t('loss.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Available' }));
  await waitFor(() => expect(calls.batch.mock.calls.at(-1)?.[0]).toMatchObject({ [`all_of_${kind}`]: 'source', to: target, environment: { use: 'destination' } }));
  await user.click(screen.getByRole('checkbox', { name: 'Second' }));
  await waitFor(() => expect(calls.batch.mock.calls.at(-1)?.[0]).toMatchObject({ thread_ids: ['first'], to: target, environment: { use: 'destination' } }));
});

it.each(['en', 'zh-CN'] as const)('searches and pages large thread and plan lists without dropping hidden selections (%s)', async (locale) => {
  await setLocale(locale);
  const threads = Array.from({ length: 30 }, (_, i) => ({ id: `Thread ${i}`, name: `Thread ${i}` }));
  const plans = threads.map((row) => resourcePlan(row.name));
  const first = plans[0]!;
  calls.batch.mockResolvedValue({ preflight_id: 'large-plan', expires_at: first.expires_at, executable: plans.length, options: first.options, estimate: first.estimate, plans, skipped: [], turns_running: 0 });
  show(<BatchChange source={{ kind: 'threads', threads, label: 'Selected' }} batch={null} open onClose={vi.fn()} />);
  const user = userEvent.setup();
  const members = await screen.findByRole('group', { name: i18n.t('change.batch.threads') });
  expect(within(members).getAllByRole('checkbox').length).toBeLessThan(threads.length);
  fireEvent.change(within(members).getByRole('searchbox'), { target: { value: 'Thread 29' } });
  expect(within(members).getByRole('checkbox', { name: 'Thread 29' })).toBeChecked();
  await user.click(screen.getByRole('combobox', { name: i18n.t('loss.destination') }));
  await user.click(await screen.findByRole('option', { name: 'Available' }));
  await waitFor(() => expect(calls.batch.mock.calls.at(-1)?.[0]).toHaveProperty('thread_ids', threads.map((thread) => thread.id)));
  const reviews = await screen.findByRole('group', { name: i18n.t('change.batch.plans') });
  expect(reviews.querySelectorAll('[data-plan-row]').length).toBeLessThan(plans.length);
  fireEvent.change(within(reviews).getByRole('searchbox'), { target: { value: 'Thread 29' } });
  expect(within(reviews).getByText('Thread 29')).toBeVisible();
  expect(reviews.querySelectorAll('[data-plan-row]')).toHaveLength(1);
  await user.click(within(members).getByRole('checkbox', { name: 'Thread 29' }));
  await waitFor(() => expect(calls.batch.mock.calls.at(-1)?.[0]).toHaveProperty('thread_ids', threads.filter((thread) => thread.id !== 'Thread 29').map((thread) => thread.id)));
});

it('searches and pages completed batch jobs in their own bounded list', async () => {
  const jobs = Array.from({ length: 30 }, (_, i) => ({ thread: resourcePlan(`Thread ${i}`).source,
    job: { ...failedStart().job, id: `job-${i}`, state: 'succeeded' as const, error: null } }));
  batchChoices.running = { batch_id: 'finished', jobs, hidden: 0, percent: 100, counts: { queued: 0, running: 0, succeeded: 30, failed: 0, cancelled: 0 } };
  show(<BatchChange source={{ kind: 'threads', threads: [], label: 'Selected' }} batch="finished" open onClose={vi.fn()} />);
  const list = await screen.findByRole('group', { name: i18n.t('change.batch.jobs') });
  expect(list.querySelectorAll('[data-batch-job]').length).toBeLessThan(jobs.length);
  await userEvent.type(within(list).getByRole('searchbox'), 'Thread 29');
  expect(within(list).getByText('Thread 29')).toBeVisible();
  expect(list.querySelectorAll('[data-batch-job]')).toHaveLength(1);
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
  await userEvent.keyboard('{Escape}');
  expect(screen.getByRole('combobox', { name: i18n.t('loss.destinationHost') })).toHaveValue(i18n.t('changeCompact.automatic'));
  expect(screen.getByRole('combobox', { name: i18n.t('loss.hostFor', { name: 'Ready' }) })).toHaveValue(i18n.t('loss.asAbove'));
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
  const coverage = i18n.t('loss.fileCoverage', { count: 4, copied: '3', total: '4', missing: '1' });
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
