/** One explicit server update control and the common durable progress/outcome rows. */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { serverRestartError, serverUpdateOpen, serverUpdateQuery, startServerUpdate } from '@/api/serverUpdate';
import { JobProgress } from '@/features/change/Progress';
import { Facts, CommandBox } from '@/features/resources/Facts';
import type { Job, ServerUpdateAvailability, ServerUpdateFailure, ServerUpdateView } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { UserChip } from '@/ui/UserChip';
import { RelativeTime } from '@/ui/RelativeTime';

const LIMITS: Record<ServerUpdateAvailability, string> = {
  ready: 'serverUpdate.unavailable.ready', no_bundled_node: 'serverUpdate.unavailable.no_bundled_node',
  node_offline: 'serverUpdate.unavailable.node_offline', docker_unavailable: 'serverUpdate.unavailable.docker_unavailable',
  ambiguous_bundled_nodes: 'serverUpdate.unavailable.ambiguous_bundled_nodes',
};
const REASONS: Record<ServerUpdateFailure, string> = {
  health_check_failed: 'serverUpdate.reasons.health_check_failed', update_failed: 'serverUpdate.reasons.update_failed', recovery_required: 'serverUpdate.reasons.recovery_required',
};
interface Props { view: ServerUpdateView; onStart: () => Promise<Job>; restarting?: boolean }
export function ServerUpdateRow({ view, onStart, restarting = false }: Props) {
  const { t } = useTranslation();
  const [accepted, setAccepted] = useState<{ job: Job; previousJobId: string | undefined } | null>(null);
  const [pending, setPending] = useState(false), busy = useRef(false), alive = useRef(true);
  const [error, setError] = useState<unknown>(null);
  const awaitingRead = accepted !== null && view.job?.id === accepted.previousJobId && view.job?.id !== accepted.job.id;
  const job = awaitingRead ? accepted.job : view.job;
  const active = serverUpdateOpen(job);
  const observation = awaitingRead ? null : view.observation;
  const restartingServer = restarting || (active && (observation?.stage === 'stopping' || observation?.stage === 'starting'));
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const start = async () => {
    if (busy.current || active || view.availability !== 'ready' || !view.update_available) return;
    busy.current = true; setPending(true); setError(null);
    try { const next = await onStart(); if (alive.current) setAccepted({ job: next, previousJobId: view.job?.id }); }
    catch (failure) { if (alive.current) setError(failure); }
    finally { busy.current = false; if (alive.current) setPending(false); }
  };
  const offer = view.update_available && view.offered_version !== null ? t('installationRecords.newer', { version: view.offered_version })
    : view.offered_version === null ? t('installationRecords.noOffer') : t('installationRecords.current');
  const outcome = observation?.stage === 'updated' ? t('serverUpdate.updated', { version: observation.to_version })
    : observation?.stage === 'rolled_back' ? t('serverUpdate.rolledBack', { version: observation.from_version ?? view.installed_version })
      : observation?.stage === 'failed' ? t('serverUpdate.failed') : null;
  return <div className="flex min-w-0 flex-col gap-3" data-server-update>
    <Facts rows={[{ name: t('installationRecords.installed'), value: view.installed_version },
      ...(view.offered_version === null ? [] : [{ name: t('componentUpdates.latestRelease'), value: view.offered_version }])]} />
    <StateLine compact title={offer} message={view.availability === 'ready' ? undefined : t(LIMITS[view.availability])} action={<span className="flex items-center gap-2">{view.availability === 'ready' && (view.update_available || active) &&
      <Button size="sm" variant="primary" pending={pending} disabled={active} onClick={() => void start()}>{t(pending || active ? 'serverUpdate.updating' : 'serverUpdate.update')}</Button>}<HelpHint term={t('serverUpdate.title')} what={t('installationRecords.releaseAbout')} guide={{ article: 'updates', heading: 'the-server' }} /></span>} />
    {restartingServer && <StateLine compact role="status" message={t('serverUpdate.restarting')} />}
    {job !== null && <><div className="flex flex-wrap items-center gap-2 text-sm text-text-muted">{job.created_by !== null && <UserChip user={job.created_by} />}<RelativeTime at={job.created_at} /></div><JobProgress job={job} heading={t('serverUpdate.title')} reconnecting={false} phaseLabel={(phase) => t(phase === 'commit' ? 'serverUpdate.phaseRequested' : 'serverUpdate.phaseUpdate')} /></>}
    {outcome !== null && <StateLine compact role="status" title={outcome} tone={observation?.stage === 'updated' ? 'neutral' : 'warning'} message={observation?.reason === null || observation?.reason === undefined ? undefined : t(REASONS[observation.reason])} />}
    {job?.state === 'failed' && observation === null && <StateLine compact tone="danger" title={t('serverUpdate.failed')} message={job.log ?? job.error?.message} />}
    {view.availability !== 'ready' && <CommandBox command={view.command} label={t('installationRecords.copyUpdate')} />}
    {observation?.reason === 'recovery_required' && <CommandBox command="./update.sh recover" label={t('installationRecords.copyUpdate')} />}
    {error !== null && <ErrorAlert error={error} />}
  </div>;
}

export function ServerUpdateSettings() {
  const { t } = useTranslation();
  const query = useQuery(serverUpdateQuery);
  const temporary = query.isError && serverRestartError(query.error);
  if (query.isError && !temporary) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === undefined) return temporary ? <StateLine compact role="status" message={t('serverUpdate.restarting')} /> : <SkeletonRows read={query} rows={2} />;
  const view = query.data;
  return <div className="flex min-w-0 flex-col gap-2">
    <ServerUpdateRow view={view} onStart={() => view.offered_version === null ? Promise.reject(new Error(t('installationRecords.noOffer'))) : startServerUpdate(view.offered_version)} restarting={temporary} />
  </div>;
}
