/** Every resource and Storage share one cleanup plan, real data fates and factual job outcomes. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cancelJob, readJob } from '@/api/exports';
import { planCleanup, startCleanup } from '@/api/storage';
import { Frame } from '@/features/change/Frame';
import { JobProgress } from '@/features/change/Progress';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import type { CleanupChoice, CleanupItem, CleanupOutcome, CleanupPlan, CleanupRequest, Job } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { RelativeTime } from '@/ui/RelativeTime';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';
import { PeopleCount } from '@/ui/PeopleList';
import { CommandBox } from '@/features/resources/Facts';
import { Dependents, RecordName } from './Storage';

const over = (job: Job) => job.state === 'succeeded' || job.state === 'failed' || job.state === 'cancelled' || job.state === 'lost';

export function Cleanup({ request, onClose, onAccepted, onFinished, onRefused }: { request: CleanupRequest; onClose: () => void; onAccepted?: (job: Job) => void; onFinished?: (job: Job) => void; onRefused?: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const client = useQueryClient();
  const finished = useRef<string | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [choices, setChoices] = useState<ReadonlyMap<string, CleanupChoice>>(new Map());
  const [typed, setTyped] = useState('');
  const [notify, setNotify] = useState<boolean | null>(null);
  const preview = useQuery({ queryKey: ['cleanup-plan', request], queryFn: () => planCleanup(request), staleTime: 0, refetchOnWindowFocus: false, retry: false });
  const plan = preview.data;
  const choice = (item: CleanupItem) => choices.get(item.record.id) ?? item.selected;
  const erases = plan?.items.some((item) => choice(item) === 'purge' || choice(item) === 'stop_and_remove') ?? false;
  const actionable = plan?.items.some((item) => choice(item) !== 'keep' && item.record.blocker === null) ?? false;
  const operatorOnly = !actionable && (plan?.items.some((item) => typeof item.record.operator_command === 'string') ?? false);
  const fate = (item: CleanupItem) => item.choices.length < 2
    ? <span className="text-sm text-text-muted">{t(`storage.cleanup.choices.${choice(item)}`)}</span>
    : <Select size="sm" hideLabel label={t('storage.cleanup.fateFor', { name: item.record.name })} value={choice(item)} options={item.choices.map((value) => ({ value, name: t(`storage.cleanup.choices.${value}`) }))} disabled={start.isPending} onValueChange={(value) => setChoices((before) => new Map(before).set(item.record.id, value))} />;
  const start = useMutation({ mutationFn: (plan: CleanupPlan) => startCleanup({ plan_id: plan.id, choices: plan.items.map((item) => ({ id: item.record.id, choice: choice(item) })), confirmation_name: erases ? typed : null, notify: notify ?? plan.notify }), onSuccess: (job) => { setJobId(job.id); onAccepted?.(job); }, onError: () => onRefused?.() });
  const progress = useQuery({ queryKey: ['cleanup-job', jobId], queryFn: ({ signal }) => readJob(jobId ?? '', signal), enabled: jobId !== null, refetchInterval: (query) => query.state.data === undefined || !over(query.state.data) ? 1_000 : false });
  const stop = useMutation({ mutationFn: () => cancelJob(jobId ?? ''), onSuccess: () => void progress.refetch() });
  const job = progress.data;
  const ended = job !== undefined && over(job);
  useEffect(() => {
    if (job === undefined || !over(job) || finished.current === job.id) return;
    finished.current = job.id;
    for (const key of ['storage', 'trace-deleted', 'deleted-inspection', 'hosts', 'mounts', 'skills', 'environments']) void client.invalidateQueries({ queryKey: [key] });
    for (const item of plan?.items ?? []) {
      const object = item.record.resource;
      if (object !== null) {
        void client.invalidateQueries({ queryKey: [object.kind === 'mcp_server' ? 'mcp-server' : object.kind, object.id] });
        if (object.kind === 'skill') void client.invalidateQueries({ queryKey: ['skill-versions', object.id] });
      }
      if (item.record.host !== null) {
        for (const key of ['host', 'host-runtimes', 'host-kept-layers']) void client.invalidateQueries({ queryKey: [key, item.record.host.id] });
      }
    }
    onFinished?.(job);
  }, [client, job, plan, onFinished]);
  const results = outcomes(job?.result);
  const records = new Map(plan?.items.map((item) => [item.record.id, item.record]));
  const at = jobId === null ? 'plan' : ended ? 'done' : 'progress';
  return <Frame open onClose={onClose} title={t('storage.clean')} at={at} steps={!operatorOnly} dismissible={!start.isPending} failed={job?.state === 'failed'} footer={<>
    <div className="mr-auto min-w-0">{jobId === null && plan !== undefined && plan.people.length + plan.other_people > 0 && <Checkbox label={<PeopleCount users={plan.people} total={plan.people.length + plan.other_people} label={t('storage.cleanup.notify', { number: formatNumber(plan.people.length + plan.other_people, locale) })} />} checked={notify ?? plan.notify} onCheckedChange={setNotify} />}</div>
    <Button disabled={start.isPending} onClick={onClose}>{t(jobId === null ? operatorOnly ? 'common.close' : 'common.cancel' : ended ? 'common.close' : 'change.progress.hide')}</Button>
    {jobId === null ? !operatorOnly && <Button variant={erases ? 'danger' : 'primary'} pending={start.isPending} disabled={plan === undefined || !actionable || erases && typed !== plan.confirmation_name} onClick={() => { if (plan !== undefined) start.mutate(plan); }}>{t('storage.clean')}</Button> : !ended && <Button pending={stop.isPending} onClick={() => stop.mutate()}>{t('storage.cleanup.cancelRemaining')}</Button>}
  </>}>
    {jobId === null ? preview.isPending ? <SkeletonRows rows={4} /> : preview.isError ? <ErrorState compact error={preview.error} onRetry={() => void preview.refetch()} /> : plan !== undefined && <>
      <div className="flex items-center justify-between gap-2"><span className="text-sm text-text-muted">{t('storage.cleanup.count', { count: plan.items.length, number: formatNumber(plan.items.length, locale) })}</span><ArticleGuideLink article="storage-cleanup" heading={operatorOnly ? 'server-files' : 'clean-up-kept-items'} title={t('storage.guide')} /></div>
      <Table label={t('storage.cleanup.plan')} boundedIdentity rows={plan.items} rowKey={(item) => item.record.id} columns={[
        { id: 'name', header: t('storage.name'), grow: true, cell: (item, shown) => <div className="flex min-w-0 flex-col gap-1"><RecordName row={item.record} /><span className="text-sm text-text-muted">{t(`storage.whys.${item.record.why}`)}{item.record.bytes !== null && ` · ${formatBytes(item.record.bytes, locale)}`}</span>{item.record.until !== null && <RelativeTime at={item.record.until} className="text-sm text-text-muted" />}{item.record.blocker !== null && <span className="text-sm text-text-muted">{t(`storage.blockers.${item.record.blocker}`, { defaultValue: item.record.blocker })}</span>}{!shown.has('dependents') && <Dependents row={item.record} />}{typeof item.record.operator_command === 'string' ? <CommandBox command={item.record.operator_command} label={t('storage.cleanup.copyOperatorCommand')} /> : !shown.has('fate') && <div className="flex flex-wrap items-center gap-2"><span className="text-sm text-text-muted">{t('storage.cleanup.fate')}</span>{fate(item)}</div>}</div> },
        ...(!operatorOnly ? [{ id: 'fate', header: t('storage.cleanup.fate'), from: 'sm' as const, cell: (item: CleanupItem) => typeof item.record.operator_command === 'string' ? null : fate(item) }] : []),
        { id: 'dependents', header: t('storage.keptBy'), from: 'md', cell: (item) => <Dependents row={item.record} /> },
      ]} />
      {plan.items.length === 0 && <StateLine title={t('storage.cleanup.empty')} />}
      {erases && <Input label={t('storage.cleanup.typeName', { name: plan.confirmation_name })} value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" disabled={start.isPending} />}
      {start.error !== null && <ErrorAlert error={start.error} />}
    </> : job === undefined ? progress.isError ? <ErrorState compact error={progress.error} onRetry={() => void progress.refetch()} /> : <SkeletonRows rows={4} /> : <>
      <JobProgress job={job} heading={t('storage.cleanup.running')} reconnecting={progress.isError} />
      {ended && <StateLine title={t(`source.job.${job.state}`)} tone={job.state === 'failed' ? 'warning' : 'neutral'} message={job.error?.message} />}
      {results.length > 0 && <Table label={t('storage.cleanup.results')} boundedIdentity rows={results} rowKey={(row) => row.id} columns={[
        { id: 'name', header: t('storage.name'), cell: (row) => { const record = records.get(row.id); return record === undefined ? <span className="truncate" title={row.id}>{row.id}</span> : <RecordName row={record} />; } },
        { id: 'outcome', header: t('storage.cleanup.outcome'), cell: (row) => <div><span>{t(`storage.cleanup.outcomes.${row.outcome}`,{defaultValue:row.outcome})}</span>{row.reason !== null && <p className="text-sm text-text-muted">{t(`storage.blockers.${row.reason}`, { defaultValue: row.reason })}</p>}</div> },
      ]} />}
      {stop.error !== null && <ErrorAlert error={stop.error} />}
    </>}
  </Frame>;
}

function outcomes(value: unknown): CleanupOutcome[] {
  if (typeof value !== 'object' || value === null || !('cleanup' in value) || !Array.isArray(value.cleanup)) return [];
  const items: unknown[] = value.cleanup;
  return items.filter((item): item is CleanupOutcome => typeof item === 'object' && item !== null && 'id' in item && typeof item.id === 'string' && 'outcome' in item && typeof item.outcome === 'string' && 'reason' in item && (item.reason === null || typeof item.reason === 'string'));
}
