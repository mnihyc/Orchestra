/** Plan size rows, progress and migration waits remain compact while their work changes. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { JobActivityLine } from '@/features/change/Progress';
import { MeasurementReason, MeasurementSize, Measurements, countsOf, waitsForCount } from '@/features/change/Measurements';
import { MountFates } from '@/features/change/MountFates';
import { MigrationLine } from '@/features/admin/InstallationProgress';
import type { MigrationObservation } from '@/api/installationProgress';
import type { FilesPlan, Job, PlanMount } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { formatSpan } from '@/lib/time';
import { show } from './support';

const state = vi.hoisted(() => ({ jobs: new Map<string, Job>(), stop: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(),
  jobQuery: (id: string) => ({ queryKey: ['live-work-job', id], queryFn: () => Promise.resolve(state.jobs.get(id)) }),
  cancelJob: state.stop,
}));
function job(): Job {
  return { id: 'count', kind: 'measurement', state: 'running', node_id: null, environment_id: null, provider_id: null, batch_id: null,
    result: null, error: null, log: null, created_by: null,
    created_at: '2026-10-24T10:00:00.000Z', started_at: '2026-10-24T10:00:00.000Z', finished_at: null,
    progress: { phase: 'measure', phases: [{ name: 'measure', state: 'running', weight: 100 }], percent: 0,
      files_done: 412000, files_total: null, bytes_done: 40000000000, bytes_total: null, rate_bps: null, eta_seconds: null, message: null,
      activity: { updated_at: '2026-10-24T10:00:05.000Z', stall_after_seconds: 30, directories_done: 9000, stopping: false } } };
}
const size = { reference: { job_id: 'count', measured_at: null }, measured: false, file_count: 0, total_bytes: 0 };
const files = { home: { ...size, measurement: size.reference }, file_states: null, private_views: [] } as unknown as FilesPlan;
const plan = { files, mounts: [], blockers: [] };
const observation: MigrationObservation = { step: 'schema', migration: 16, description: 'Background counts', migrations_done: 2, migrations_total: 5,
  rows_done: 1204000, rows_total: null, detail: 'building index', elapsed_seconds: 130, retry_after_seconds: 1, updated_at: '2026-10-24T10:00:05.000Z' };

afterEach(async () => { vi.useRealTimers(); state.jobs.clear(); state.stop.mockReset(); await setLocale('en'); });

const checkedMount: PlanMount = {
  mount: { id: 'checked', name: 'datasets', address: 'acme.datasets' }, action: 'keep', path: '/datasets', mode: 'read_only', detached_because: null,
  fate: 'as_is', default_fate: 'as_is', fates: ['as_is'], copy_unavailable: 'unchanged', copy: null, data_kind: 'own',
  fingerprints_available: true, fingerprints: null, fingerprint_measurement: { job_id: 'fingerprint', measured_at: null },
};

for (const locale of ['en', 'zh-CN'] as const) it(`keeps a live content check, its stalled phase and stop beside the mount in ${locale}`, async () => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  const updated = new Date(Date.now() - 31_000).toISOString();
  const value: Job = { ...job(), id: 'fingerprint', progress: { ...job().progress, phase: 'fingerprint_target', activity: { ...job().progress.activity!, updated_at: updated } } };
  state.jobs.set(value.id, value);
  state.stop.mockResolvedValue({ ...value, progress: { ...value.progress, activity: { ...value.progress.activity!, stopping: true } } });
  const retry = vi.fn();
  const rendered = show(<Measurements plan={{ mounts: [checkedMount] }} active onReady={vi.fn()}><MountFates mounts={[checkedMount]} picks={{}} onPick={vi.fn()} onCheck={retry} /></Measurements>);
  await screen.findByText((text) => text.startsWith(t('change.phase.fingerprint_target.doing')));
  expect(rendered.container.querySelector('[data-job-stalled]')).not.toBeNull();
  expect(rendered.container.querySelector('[data-job-line]')).toHaveTextContent(t('liveWork.files', { count: value.progress.files_done, value: '412,000' }));
  expect(screen.getByRole('button', { name: t('mountPlan.fingerprints.checking') })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: t('mountPlan.fingerprints.checking') }));
  expect(retry).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: t('mountPlan.fingerprints.stop') }));
  await waitFor(() => expect(state.stop).toHaveBeenCalledWith(value.id));
  await screen.findByText(t('liveWork.stopping'));
  expect(screen.getByRole('button', { name: t('liveWork.stopping') })).toBeDisabled();
  expect(retry).not.toHaveBeenCalled();
});

it('shows a completed comparison before another count finishes and lets a required count settle after a check stops', async () => {
  const at = new Date().toISOString();
  state.jobs.set('fingerprint', { ...job(), id: 'fingerprint', state: 'succeeded', finished_at: at, result: { matches: false, checked_at: at } });
  state.jobs.set('count', job());
  const ready = vi.fn();
  const rendered = show(<Measurements plan={{ ...plan, mounts: [checkedMount] }} active onReady={ready}><MountFates mounts={[checkedMount]} picks={{}} onPick={vi.fn()} onCheck={vi.fn()} /><MeasurementSize size={size} /></Measurements>);
  await screen.findByText(/Differs · checked/);
  expect(screen.getByRole('status').querySelector('time')).toHaveAttribute('datetime', at);
  expect(screen.getByText('Counting… 412,000 files · 37.3 GiB')).toBeVisible();
  await waitFor(() => expect(ready).toHaveBeenCalledTimes(1));
  rendered.unmount();
  state.jobs.set('fingerprint', { ...job(), id: 'fingerprint', state: 'cancelled' });
  state.jobs.set('count', { ...job(), state: 'succeeded', result: { file_count: 5, total_bytes: 8192 } });
  const countReady = vi.fn();
  const retry = vi.fn();
  show(<Measurements plan={{ ...plan, mounts: [checkedMount] }} active onReady={countReady}><MountFates mounts={[checkedMount]} picks={{}} onPick={vi.fn()} onCheck={retry} /></Measurements>);
  await screen.findByText('Check stopped');
  await waitFor(() => expect(countReady).toHaveBeenCalledTimes(1));
  await userEvent.click(screen.getByRole('button', { name: 'Check' }));
  expect(retry).toHaveBeenCalledTimes(1);
  expect(screen.queryByText(/Same content|Differs/)).not.toBeInTheDocument();
});

it('retains a failed read reason and prevents another row from duplicating an active check', async () => {
  const at = new Date().toISOString();
  const failed: Job = { ...job(), id: 'fingerprint', state: 'failed', error: { code: 'unsupported', phase: 'fingerprint_source', undone: true, message: 'This host cannot read the data' } };
  state.jobs.set(failed.id, failed);
  const ready = vi.fn();
  const retry = vi.fn();
  const rendered = show(<Measurements plan={{ mounts: [checkedMount] }} active onReady={ready}><MountFates mounts={[checkedMount]} picks={{}} onPick={vi.fn()} onCheck={retry} /></Measurements>);
  await screen.findByRole('alert');
  expect(screen.getByRole('alert')).toHaveTextContent('Could not check · This host cannot read the data');
  expect(screen.getAllByRole('button', { name: 'Check' })).toHaveLength(1);
  expect(ready).not.toHaveBeenCalled();
  rendered.unmount();
  const second: PlanMount = { ...checkedMount, mount: { ...checkedMount.mount, id: 'other', name: 'other data' }, fingerprint_measurement: { job_id: 'second', measured_at: null } };
  state.jobs.set('fingerprint', { ...job(), id: 'fingerprint' });
  state.jobs.set('second', { ...job(), id: 'second', state: 'succeeded', finished_at: at, result: { checked_at: at, matches: true } });
  show(<Measurements plan={{ mounts: [checkedMount, second] }} active onReady={vi.fn()}><MountFates mounts={[checkedMount, second]} picks={{}} onPick={vi.fn()} onCheck={retry} /></Measurements>);
  await screen.findByText(/Same content · checked/);
  const check = screen.getByRole('button', { name: 'Check' });
  expect(check).toBeDisabled();
  await userEvent.click(check);
  expect(retry).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) for (const outcome of ['failed', 'cancelled', 'lost', 'request_failed'] as const) it(`prevents duplicate reads while a ${outcome} check retries in ${locale}`, async () => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  const native = 'This host cannot read the data';
  const mount = outcome === 'request_failed' ? { ...checkedMount, fingerprint_measurement: undefined } : checkedMount;
  const problem = outcome === 'request_failed' ? new Error(native) : null;
  if (outcome !== 'request_failed') state.jobs.set('fingerprint', { ...job(), id: 'fingerprint', state: outcome,
    error: outcome === 'failed' ? { code: 'unsupported', phase: 'fingerprint_source', undone: true, message: native } : null });
  const retry = vi.fn();
  const ready = vi.fn();
  const view = (checking: boolean) => <Measurements plan={{ mounts: [mount] }} active onReady={ready}><MountFates mounts={[mount]} picks={{}} onPick={vi.fn()} onCheck={retry} checking={checking} checkError={problem} /></Measurements>;
  const rendered = show(view(false));
  const check = await screen.findByRole('button', { name: t('mountPlan.fingerprints.check') });
  expect(check).toBeEnabled();
  if (outcome === 'lost') { expect(screen.getByRole('status')).toHaveTextContent(t('source.job.lost')); expect(ready).not.toHaveBeenCalled(); }
  await userEvent.click(check);
  expect(retry).toHaveBeenCalledTimes(1);
  rendered.rerender(view(true));
  const pending = screen.getByRole('button', { name: t('mountPlan.fingerprints.checking') });
  expect(pending).toBeDisabled();
  await userEvent.dblClick(pending);
  expect(retry).toHaveBeenCalledTimes(1);
  rendered.rerender(view(false));
  const again = screen.getByRole('button', { name: t('mountPlan.fingerprints.check') });
  expect(again).toBeEnabled();
  await userEvent.click(again);
  expect(retry).toHaveBeenCalledTimes(2);
});

it('keeps counts, elapsed and the dated stalled fragment on the same line with directories in its title', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-24T10:00:20.000Z'));
  const rendered = show(<JobActivityLine job={job()} />);
  const line = rendered.container.querySelector('[data-job-activity]')!;
  expect(line).toHaveTextContent('Counting · 412,000 files · 37.3 GiB · 0:20');
  expect(line).toHaveAttribute('title', '9,000 directories');
  expect(line.textContent).not.toContain('directories');
  expect(line.querySelector('p')).toBeNull();
  expect(rendered.container.querySelector('[data-job-stalled]')).toBeNull();
  act(() => { vi.advanceTimersByTime(16000); });
  expect(line).toHaveTextContent('0:36 · no change since');
  rendered.rerender(<JobActivityLine job={{ ...job(), progress: { ...job().progress, activity: { ...job().progress.activity!, updated_at: '2026-10-24T10:00:36.000Z' } } }} />);
  expect(rendered.container.querySelector('[data-job-stalled]')).toBeNull();
});

it('shows a scheduled model recovery as its known wait before ordinary stall detection resumes', () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-24T10:01:00.000Z'));
  const value = job();
  const rendered = show(<JobActivityLine job={{ ...value, call_recovery: {
    state: 'scheduled', failure: { code: 'provider_busy', message: 'Synthetic capacity', retryable: true, window: null, said: null, resets_at: null, incomplete: false },
    rule_id: 'busy', step: { type: 'retry', delay_secs: 60, times: 3, backoff: 2 }, attempt: 1,
    due_at: '2026-10-24T10:02:00.000Z', detail: null, count: null, window_secs: null,
  } }} />);
  expect(rendered.container.querySelector('[data-job-line]')).toHaveTextContent('Provider busy');
  expect(rendered.container.querySelector('[data-job-elapsed]')).toBeVisible();
  expect(rendered.container.querySelector('[data-job-stalled]')).toBeNull();
  rendered.rerender(<JobActivityLine job={value} />);
  expect(rendered.container.querySelector('[data-job-stalled]')).toBeVisible();
});

it('shows rate and time left only for copies, with elapsed as a duration beyond a day', () => {
  const value=job();
  value.kind='environment_move'; value.progress.phase='copy'; value.progress.files_total=980000;
  value.progress.bytes_total=80000000000; value.progress.rate_bps=50331648; value.progress.eta_seconds=300;
  const rendered=show(<JobActivityLine job={value} />);
  expect(rendered.container.querySelector('[data-job-line]')).toHaveTextContent('Copying · 412,000 of 980,000 files · 37.3 GiB of 74.5 GiB · ≈48 MiB/s · ~5 min left');
  value.kind='measurement'; value.progress.phase='measure';
  rendered.rerender(<JobActivityLine job={value} />);
  expect(rendered.container.querySelector('[data-job-line]')?.textContent).not.toContain('/s');
  expect(formatSpan(26*3600000+61000,{locale:'en'},'clock')).toBe('26:01:01');
});

it('keeps stopping visible and names a machine only after the configured stop wait', () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-24T10:00:20.000Z'));
  const value=job(); value.progress.activity={...value.progress.activity!,stopping:true,waiting_for:{type:'machine',name:'host-2'}};
  const rendered=show(<JobActivityLine job={value} />);
  expect(screen.getByText('Stopping…')).toBeVisible();
  act(() => {vi.advanceTimersByTime(16000);});
  expect(screen.getByText('Still stopping on host-2')).toBeVisible();
  expect(rendered.container.querySelectorAll('[data-job-activity]')).toHaveLength(1);
});

it('names an environment change that holds file work before a machine command starts', () => {
  const value=job(); value.progress.activity={...value.progress.activity!,waiting_for:{type:'environment_change'}};
  show(<JobActivityLine job={value} />);
  expect(screen.getByText('Queued behind another change')).toBeVisible();
});

it('puts live counts and Stop counting in the size row and the count wait beside confirmation', async () => {
  state.jobs.set('count',job()); state.stop.mockResolvedValue({...job(),progress:{...job().progress,activity:{...job().progress.activity!,stopping:true}}});
  const rendered=show(<Measurements plan={plan} active onReady={vi.fn()}><span>Home · <MeasurementSize size={size} /></span><MeasurementReason waiting planning={false} fallback={null} /></Measurements>);
  await screen.findByText('Counting… 412,000 files · 37.3 GiB');
  expect(screen.getByText('Counting 1 of 1 location')).toBeVisible();
  expect(rendered.container.querySelector('[data-counting-size]')).toHaveAttribute('title','9,000 directories');
  await userEvent.click(screen.getByRole('button',{name:'Stop counting'}));
  await screen.findByText('Stopping…');
  expect(state.stop).toHaveBeenCalledWith('count');
  expect(screen.getByRole('button',{name:'Stopping…'})).toBeDisabled();
});

for(const outcome of ['failed','cancelled'] as const) it(`offers Retry in a ${outcome} count row without an automatic restart`, async () => {
  state.jobs.set('count',{...job(),state:outcome}); const refresh=vi.fn();
  show(<Measurements plan={plan} active onReady={refresh}><MeasurementSize size={size} /></Measurements>);
  expect(await screen.findByText(outcome==='failed'?'Count failed':'Count stopped')).toBeVisible();
  expect(refresh).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button',{name:'Retry'})); expect(refresh).toHaveBeenCalledOnce();
});

it('replaces the size and replans once when its last background count succeeds', async () => {
  state.jobs.set('count',{...job(),state:'succeeded',result:{file_count:4,total_bytes:8192}});
  const refresh=vi.fn();
  const rendered=show(<Measurements plan={plan} active onReady={refresh}><MeasurementSize size={size} /><MeasurementReason waiting={false} planning={false} fallback={null} /></Measurements>);
  await waitFor(()=>expect(refresh).toHaveBeenCalledOnce());
  expect(screen.getByText('4 files · 8 KiB')).toBeVisible();
  expect(screen.queryByRole('button',{name:'Stop counting'})).toBeNull();
  expect(countsOf(plan)).toHaveLength(1); expect(waitsForCount({blockers:[{code:'measurement_pending'}]})).toBe(true);
  rendered.rerender(<Measurements plan={plan} active onReady={refresh}><MeasurementSize size={size} /></Measurements>);
  expect(refresh).toHaveBeenCalledOnce();
});

it('shows a finished volume count while the other location is still counting', async () => {
  state.jobs.set('count', { ...job(), state: 'succeeded', result: { volumes: [{ selector: 'home', measure: { file_count: 4, total_bytes: 8192 } }] } });
  state.jobs.set('another', { ...job(), id: 'another' });
  const another = { ...size, reference: { job_id: 'another', measured_at: null } };
  const pendingPlan = { ...plan, files: { ...files, file_states: { ...another, measurement: another.reference } } } as unknown as typeof plan;
  const refresh = vi.fn();
  show(<Measurements plan={pendingPlan} active onReady={refresh}><MeasurementSize size={size} /><MeasurementSize size={another} /></Measurements>);
  expect(await screen.findByText('4 files · 8 KiB')).toBeVisible();
  expect(await screen.findByText('Counting… 412,000 files · 37.3 GiB')).toBeVisible();
  expect(screen.getAllByRole('button', { name: 'Stop counting' })).toHaveLength(1);
  expect(refresh).not.toHaveBeenCalled();
});

it('uses one migration line in the banner and keeps native detail in its title', () => {
  const rendered=show(<MigrationLine observation={observation} elapsed={130000} />);
  const line=rendered.container.querySelector('[data-migration-progress]');
  expect(line).toHaveTextContent('Updating database · migration 0016 (3 of 5) · 1,204,000 rows · 2:10');
  expect(line).toHaveAttribute('title','Background counts · building index');
  expect(rendered.container.querySelector('p')).toBeNull();
  rendered.rerender(<MigrationLine observation={{...observation,step:'failed'}} elapsed={130000} />);
  expect(line).toHaveTextContent("Database update failed · see the server's update log");
  rendered.rerender(<MigrationLine observation={{...observation,step:'complete'}} elapsed={130000} />);
  expect(line).toHaveTextContent('Database updated · reconnecting…');
});

it('offers Stop counting for a new count after stopping an earlier count in the same plan row', async () => {
  state.jobs.set('count', job()); state.stop.mockResolvedValue({...job(),state:'cancelled'});
  const refresh=vi.fn();
  const rendered=show(<Measurements plan={plan} active onReady={refresh}><MeasurementSize size={size}/></Measurements>);
  await userEvent.click(await screen.findByRole('button',{name:'Stop counting'}));
  await waitFor(()=>expect(state.stop).toHaveBeenCalledWith('count'));
  await screen.findByRole('button',{name:'Stopping…'});
  const nextSize={...size,reference:{job_id:'retry-count',measured_at:null}};
  state.jobs.set('retry-count',{...job(),id:'retry-count'});
  const nextPlan={...plan,files:{...files,home:{...files.home!,measurement:nextSize.reference}}};
  rendered.rerender(<Measurements plan={nextPlan} active onReady={refresh}><MeasurementSize size={nextSize}/></Measurements>);
  await screen.findByText('Counting… 412,000 files · 37.3 GiB');
  expect(screen.getByRole('button',{name:'Stop counting'})).not.toBeDisabled();
});


for (const locale of ['en','zh-CN'] as const) it(`names the host deletion's recovery, manager and frozen-data waits in ${locale}`, async()=>{
  await setLocale(locale);
  const t=i18n.getFixedT(locale);
  const base=job();
  const activity={updated_at:new Date().toISOString(),stall_after_seconds:0,directories_done:null,stopping:false};
  let rendered=show(<JobActivityLine job={{...base,kind:'host_delete',progress:{...base.progress,phase:'cascades',activity:{...activity,waiting_for:{type:'host_environments',count:4}}}}}/>);
  expect(screen.getByText((text)=>text.startsWith(t('hostCustody.waitOthers',{count:4,number:'4'})))).toBeVisible();
  rendered.unmount();
  rendered=show(<JobActivityLine job={{...base,kind:'host_delete',progress:{...base.progress,phase:'cascades',activity:{...activity,waiting_for:{type:'operations',count:2}}}}}/>);
  expect(screen.getByText((text)=>text.startsWith(t('hostCustody.waitOperations',{count:2,number:'2'})))).toBeVisible();
  rendered.unmount();
  rendered=show(<JobActivityLine job={{...base,kind:'host_delete',progress:{...base.progress,phase:'cascades',activity:{...activity,waiting_for:{type:'retained_data',until:'2026-11-03T12:00:00.000Z'}}}}}/>);
  expect(rendered.container.querySelector('[data-job-line]')).toHaveTextContent(locale==='en'?'Waiting for kept data until':'等待保留数据到期');
  expect(rendered.container.querySelector('[data-job-line]')?.textContent).not.toContain('2026-11-03T');
  rendered.unmount();
  rendered=show(<JobActivityLine job={{...base,kind:'host_delete',progress:{...base.progress,phase:'cascades',activity:{...activity,waiting_for:{type:'machine',name:'host-2'}}}}}/>);
  expect(screen.getByText(t('liveWork.waitingFor',{machine:'host-2'}))).toBeVisible();
  rendered.unmount();
});
