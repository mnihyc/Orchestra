/** Mount choices share the plan's compact table: its existing name, a small choice and one size fragment. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { X } from 'lucide-react';
import { Trans, useTranslation } from 'react-i18next';

import { record } from '@/api/cache';
import { blockersOf, cancelJob, forkKeys } from '@/api/forks';
import type { MountPick } from '@/features/change/choices';
import { MeasurementSize, usePlanMeasurements } from '@/features/change/Measurements';
import { Hint } from '@/features/help/Hint';
import { JobActivityLine } from '@/features/change/Progress';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { CopyInto, CopyUnavailable, Job, MountFate, PlanMount, PlanMountFingerprints } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import type { MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { StateLine } from '@/ui/StateLine';
import { Table, type Column } from '@/ui/Table';

/** The shared table moves these facts under the identity when their columns no longer fit. */
const CHOICE_COLUMN = 'choice';
const SIZE_COLUMN = 'size';
const FATES: readonly MountFate[] = ['detach', 'as_is', 'copy'];
interface Row { mount: PlanMount; pick: MountPick | undefined; fate: MountFate; copy: PlanMount['copy'] }

function reason(why: CopyUnavailable | null, t: TFunction): string | undefined {
  return why === null ? undefined : t(`mountPlan.copyUnavailable.${why}`);
}

function observed(job: Job | undefined): PlanMountFingerprints | null {
  const result = job?.state === 'succeeded' ? job.result : null;
  return record(result) && typeof result.matches === 'boolean' && typeof result.checked_at === 'string'
    ? { matches: result.matches, checked_at: result.checked_at } : null;
}

function FingerprintCheck({ mount, onCheck, checking, disabled, error, stopRequested }: {
  mount: PlanMount; onCheck: () => void; checking: boolean; disabled: boolean; error: Error | null; stopRequested: boolean;
}) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const counts = usePlanMeasurements();
  const reference = mount.fingerprint_measurement;
  const view = reference === undefined || reference === null ? undefined : counts?.views.get(reference.job_id);
  const job = view?.job;
  const stop = useMutation({ mutationFn: (id: string) => cancelJob(id), onSuccess: (value) => cache.setQueryData(forkKeys.job(value.id), value) });
  const running = reference?.measured_at === null && view?.failed !== true
    && (job === undefined || job.state === 'queued' || job.state === 'running');
  const stopping = stopRequested || reference !== undefined && reference !== null && stop.variables === reference.job_id && (stop.isPending || stop.isSuccess) || job?.progress.activity?.stopping === true;
  const button = <Button variant="ghost" size="sm" pending={checking || running} disabled={disabled || checking || running} onClick={onCheck}>{t(checking || running ? 'mountPlan.fingerprints.checking' : 'mountPlan.fingerprints.check')}</Button>;
  const result = mount.fingerprints ?? observed(job);
  const failed = error !== null || view?.failed === true || job?.state === 'failed' || job?.state === 'succeeded' && result === null;
  if (failed) {
    const failure = error ?? view?.error ?? job?.error;
    const native = error !== null ? blockersOf(error).find((blocker) => blocker.code === 'mount_fingerprint_unavailable')?.message ?? error.message : job?.error?.message;
    return <ErrorAlert error={failure} message={<>{t('mountPlan.fingerprints.failed')}{native !== undefined && <> · {native}</>}</>} action={button} className="mt-1 min-w-0 whitespace-normal" />;
  }
  if (job?.state === 'lost') return <StateLine compact tone="warning" role="status" message={t('source.job.lost')} action={button} />;
  if (job?.state === 'cancelled') return <StateLine compact role="status" message={t('mountPlan.fingerprints.stopped')} action={button} className="text-text-muted" />;
  return <div className="mt-1 flex min-w-0 flex-wrap items-center gap-1 whitespace-normal" data-fingerprint-check>
    {button}
    {running && <IconButton size="sm" action="cancel" icon={<X aria-hidden />} label={t(stopping ? 'liveWork.stopping' : 'mountPlan.fingerprints.stop')} pending={stop.isPending && stop.variables === reference.job_id} disabled={stopping} reason={stop.isError ? t('liveWork.stopFailed') : undefined} onClick={() => stop.mutate(reference.job_id)} />}
    {running && job !== undefined && <div className="basis-full min-w-0"><JobActivityLine job={stop.variables === reference.job_id && stop.isSuccess && stop.data.progress.activity?.stopping === true ? stop.data : job} /></div>}
    {!checking && !running && result !== null && <span className="basis-full text-sm text-text-muted tabular-nums" role="status">
      {t(result.matches ? 'mountPlan.fingerprints.same' : 'mountPlan.fingerprints.differs')}{' · '}
      <Trans i18nKey="mountPlan.fingerprints.checked" components={{ time: <RelativeTime at={result.checked_at} /> }} />
    </span>}
  </div>;
}

export function MountFates({ mounts, picks, onPick, onPickAll, onCheck, checking = false, checkDisabled = false, checkError = null }: {
  mounts: readonly PlanMount[];
  picks: Readonly<Record<string, MountPick>>;
  onPick: (mount: string, pick: MountPick | null) => void;
  onPickAll?: (fate: MountFate | null) => void;
  onCheck?: () => void;
  checking?: boolean;
  checkDisabled?: boolean;
  checkError?: Error | null;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const counts = usePlanMeasurements();
  const cache = useQueryClient();
  const stopCheck = useMutation({ mutationFn: (id: string) => cancelJob(id), onSuccess: (value) => cache.setQueryData(forkKeys.job(value.id), value) });
  const rows: Row[] = mounts.filter((mount) => mount.fates.length > 1 || mount.fingerprints_available).map((mount) => {
    const pick = picks[mount.mount.id];
    const fate = pick?.fate ?? mount.fate;
    return { mount, pick, fate, copy: fate === 'copy' ? mount.copy : null };
  });
  if (rows.length === 0) return null;
  // The request checks the eligible rows together, so a second click must not duplicate another row's active read.
  const anyReading = rows.some(({ mount }) => {
    const reference = mount.fingerprint_measurement;
    if (reference?.measured_at !== null) return false;
    const view = counts?.views.get(reference.job_id);
    return view?.failed !== true && (view?.job === undefined || view.job.state === 'queued' || view.job.state === 'running');
  });
  const size = ({ copy, fate }: Row) => fate !== 'copy' ? null : copy?.measurement !== undefined && copy.measurement !== null
    ? <MeasurementSize size={{ reference: copy.measurement, measured: copy.measured, file_count: copy.file_count, total_bytes: copy.total_bytes }} />
    : <span className="text-sm text-text-muted tabular-nums">{copy?.measured === true
      ? t('liveWork.files', { count: copy.file_count, value: formatNumber(copy.file_count, locale) }) + ' · ' + formatBytes(copy.total_bytes, locale)
      : t('liveWork.notCounted')}</span>;
  const choose = (mount: PlanMount, next: MountFate) => onPick(mount.mount.id, next === mount.default_fate ? null : { fate: next, into: null });
  const choice = ({ mount, pick, fate, copy }: Row) => mount.fates.length <= 1 ? null : <div className="flex flex-col gap-1">
    <Select<MountFate> size="sm" hideLabel className="w-36" label={t('mountPlan.fates.of', { mount: mount.mount.name })} value={fate} onValueChange={(next) => choose(mount, next)}
      options={FATES.map((value) => ({ value, name: t(`choiceCatalog.mountFate.${value}.name`), disabled: !mount.fates.includes(value), description: value === 'copy' ? reason(mount.copy_unavailable, t) : undefined }))} />
    {copy?.holds !== undefined && copy.holds !== null && <Select<CopyInto | 'unanswered'> size="sm" hideLabel className="w-36" label={t('mountPlan.fates.into', { mount: mount.mount.name })} value={pick?.into ?? copy.into ?? 'unanswered'}
      onValueChange={(into) => { if (into !== 'unanswered') onPick(mount.mount.id, { fate: 'copy', into }); }}
      options={[
        { value: 'unanswered', name: t('liveWork.choose'), disabled: true },
        { value: 'replace', name: t('mountPlan.fates.replace'), disabled: !copy.replace, description: copy.replace ? undefined : t('mountPlan.fates.replaceUnavailable') },
        { value: 'new_location', name: t('mountPlan.fates.keepBoth') },
      ]} />}
  </div>;
  const entries = ({ mount, fate }: Row): MenuEntry[] => {
    const values: MenuEntry[] = [];
    if (mount.fates.length > 1) values.push({ type: 'radio', id: 'fate', label: t('mountPlan.fates.of', { mount: mount.mount.name }), value: fate,
      onValueChange: (value) => { const next = FATES.find((candidate) => candidate === value); if (next !== undefined && mount.fates.includes(next)) choose(mount, next); },
      options: FATES.filter((value) => mount.fates.includes(value)).map((value) => ({ value, name: t(`choiceCatalog.mountFate.${value}.name`) })) });
    const reference = mount.fingerprint_measurement;
    const view = reference === undefined || reference === null ? undefined : counts?.views.get(reference.job_id);
    const reading = reference?.measured_at === null && view?.failed !== true && (view?.job === undefined || view.job.state === 'queued' || view.job.state === 'running');
    if (mount.fingerprints_available && onCheck !== undefined) values.push({ id: 'check', name: t('mountPlan.fingerprints.check'), onSelect: onCheck, disabled: checking || anyReading ? t('mountPlan.fingerprints.checking') : checkDisabled ? t('changeReady.checking') : false });
    if (reading) {
      const stopping = view?.job?.progress.activity?.stopping === true || stopCheck.variables === reference.job_id && (stopCheck.isPending || stopCheck.isSuccess);
      values.push({ id: 'stop-check', name: t('mountPlan.fingerprints.stop'), disabled: stopping ? t('liveWork.stopping') : false, onSelect: () => stopCheck.mutate(reference.job_id) });
    }
    return values;
  };
  const columns: Column<Row>[] = [
    { id: 'name', header: t('resources.listName'), grow: true, cell: (row, shown) => <div className="min-w-0" data-mount-fate={row.mount.mount.name}>
      <div className="flex min-w-0 items-center justify-between gap-2">
        <div className="min-w-0 flex-1"><ResourceChip subject={{ kind: 'mount', mount: row.mount.mount }} hint={false} /></div>
        {!shown.has(CHOICE_COLUMN) && choice(row)}
      </div>
      {!shown.has(SIZE_COLUMN) && <div className="mt-1 min-w-0">{size(row)}</div>}
      {row.mount.fingerprints_available && onCheck !== undefined && <FingerprintCheck mount={row.mount} onCheck={onCheck} checking={checking} disabled={checkDisabled || anyReading} error={checkError} stopRequested={row.mount.fingerprint_measurement?.job_id === stopCheck.variables && (stopCheck.isPending || stopCheck.isSuccess)} />}
    </div> },
    { id: CHOICE_COLUMN, header: t('liveWork.action'), from: 'sm', cell: choice },
    { id: SIZE_COLUMN, header: t('liveWork.size'), from: 'md', cell: size },
  ];
  const changeable = rows.filter((row) => row.mount.fates.length > 1);
  const shared = changeable.every((row) => row.fate === changeable[0]?.fate) ? changeable[0]?.fate ?? 'per_mount' : 'per_mount';
  return <div className="flex min-w-0 flex-col gap-2" data-mount-fates>
    <Hint id="mount-fates" popover className="self-start" />
    {onPickAll !== undefined && changeable.length > 0 && <Select<MountFate | 'per_mount'> size="sm" label={t('liveWork.allMounts')} value={shared} onValueChange={(value) => onPickAll(value === 'per_mount' ? null : value)}
      options={[{ value: 'per_mount', name: t('liveWork.perMount') }, ...FATES.map((value) => ({ value, name: t(`choiceCatalog.mountFate.${value}.name`), disabled: !changeable.every((row) => row.mount.fates.includes(value)) }))]} />}
    <Table label={t('mountPlan.fates.label')} columns={columns} rows={rows} rowKey={(row) => row.mount.mount.id} boundedIdentity rowMenu={{ label: (row) => row.mount.mount.name, entries }} />
  </div>;
}
