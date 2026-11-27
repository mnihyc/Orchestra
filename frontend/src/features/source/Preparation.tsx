/** Where a thread's files come from, while they are being made ready: the repository, branch and directory from the server's facts, the job's progress, and why a preparation failed, with Prepare again.
 *
 * A thread whose source is a repository or a worktree on a new branch holds its first message until the
 * source is ready; this says so above the conversation. A directory that was there already is kept
 * on every machine. Nothing here guesses a branch or an address.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Folder, FolderGit2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { isOver, jobQuery } from '@/api/forks';
import { prepareSource } from '@/api/source';
import { JobActivityLine } from '@/features/change/Progress';
import { workspaceDenial } from '@/features/files/door';
import type { SourceInfo, Thread } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Meter } from '@/ui/Meter';
import { SkeletonRows } from '@/ui/Skeleton';
import { preparationFailure, sourceFailure } from './words';
import { WorkspaceInstructions } from '@/features/start/WorkspaceInstructions';

/** Preparing a repository needs management of its environment. */
const PREPARE_PERMISSION = 'environment.manage';

/** Where a source comes from, as one line: the repository and branch, or the directory that was there. */
export function sourceLine(t: TFunction, source: SourceInfo): string {
  const directory = source.directory === '' || source.directory === '/' ? t('source.prepare.workspace') : source.directory;
  if (source.kind === 'directory') return t('source.prepare.existing', { directory });
  const repository = source.repository_url ?? t('source.prepare.unknownRepository');
  const branch = source.branch ?? t('source.prepare.defaultBranch');
  return source.kind === 'worktree' ? t('source.prepare.worktree', { repository, branch, directory }) : t('source.prepare.repository', { repository, branch, directory });
}

/** The banner of a source that is not ready: nothing at all once it is. */
export function SourcePreparation({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const key = useRef(newActKey());
  const [jobDetails, setJobDetails] = useState<string | null>(null);
  const environment = thread.environment;
  const again = useMutation({ mutationFn: () => prepareSource(environment?.id ?? '', thread.id, key.current) });
  const source = thread.source;
  // A source in an environment that failed is never prepared: the thread's environment line says why and the ways on.
  if (source === null || source.state === 'ready' || environment === null || environment.status === 'failed' || thread.state === 'blocked' && thread.blocked?.reason === 'environment_unavailable') return null;
  const mayPrepare = thread.environment_permissions.includes(PREPARE_PERMISSION);
  const job = source.job;
  const details = job === null ? undefined : <Button action="show" size="sm" variant="ghost" onClick={() => setJobDetails(job.id)}>{t('source.prepare.details')}</Button>;
  const failedWhy = source.state === 'failed' ? failureReason(t, source, mayPrepare, environment) : null;
  const panel = jobDetails !== null && job?.id === jobDetails && <PreparationJob id={jobDetails} source={source} failedWhy={failedWhy} onClose={() => setJobDetails(null)} />;
  if (source.state === 'failed') {
    return (
      <>
      <InlineAlert
        tone="danger"
        title={t('source.prepare.failed')}
        action={
          <span className="flex flex-wrap items-center gap-2">
            {details}
          {mayPrepare && (
            <Button action="retry" size="sm" pending={again.isPending} onClick={() => again.mutate()}>
              {source.reason === 'clone_stalled' ? t('source.prepare.retry') : t('source.prepare.again')}
            </Button>
          )}
          </span>
        }
      >
        <span className="flex flex-col gap-1" data-source-state="failed">
          <span className="wrap-anywhere">{sourceLine(t, source)}</span>
          {source.reason !== null && <span className="wrap-anywhere">{preparationFailure(t, source)}</span>}
          <span>{t('source.prepare.held')}</span>
          <WorkspaceInstructions environmentId={environment.id} directory={thread.directory} recovery={{thread:thread.id,state:JSON.stringify([source.state,source.job?.id,source.job?.state])}}/>
          {!mayPrepare && <span className="text-text-muted">{workspaceDenial(t, PREPARE_PERMISSION, environment)}</span>}
          {again.error !== null && <ErrorAlert error={again.error} />}
        </span>
      </InlineAlert>
      {panel}
      </>
    );
  }
  return (
    <>
    <InlineAlert tone="info" title={source.state === 'pending' ? t('source.prepare.pending') : t('source.prepare.preparing')} action={details}>
      <span className="flex flex-col gap-1" data-source-state={source.state}>
        <span className="inline-flex items-center gap-1 wrap-anywhere">
          {/* A plain directory is not drawn as a repository: nothing about it needs Git. */}
          {source.kind === 'directory' ? <Folder aria-hidden className="size-4 shrink-0" /> : <FolderGit2 aria-hidden className="size-4 shrink-0" />}
          {sourceLine(t, source)}
        </span>
        {source.job !== null && <Meter look="progress" value={source.job.percent} label={t('source.prepare.preparing')} detail={source.job.phase} />}
        <span>{t('source.prepare.held')}</span>
      </span>
    </InlineAlert>
    {panel}
    </>
  );
}

/** The steps a preparation goes through, by the names its job gives them. */
const STEPS = ['validate', 'apply', 'verify', 'files', 'commit'] as const;

/** The name of the step a preparation stopped at. */
function stepName(t: TFunction, phase: string): string {
  const step = STEPS.find((known) => known === phase);
  return step === undefined ? t('source.step.other') : t(`source.step.${step}`);
}

/** Why a preparation failed, in the reader's words: whoever may not prepare it is told whom to ask, as the line above
 * the conversation tells them; otherwise the sentence of the failure's code. */
function failureReason(t: TFunction, source: SourceInfo, mayPrepare: boolean, environment: NonNullable<Thread['environment']>): string {
  if (source.reason === 'access.denied' && !mayPrepare) return workspaceDenial(t, PREPARE_PERMISSION, environment);
  return preparationFailure(t, source);
}

/**
 * A short source reference grants no diagnostic body; opening it makes the current authorized job read. A failed one
 * says the step it stopped at and why: from the job's error where the reader may read it, otherwise from what the
 * thread's own source says (its code), which is what its line above the conversation says too.
 */
function PreparationJob({ id, source, failedWhy, onClose }: { id: string; source: SourceInfo; failedWhy: string | null; onClose: () => void }) {
  const { t } = useTranslation();
  const read = useQuery(jobQuery(id));
  const job = read.data;
  const step = [job?.error?.phase, job?.progress.phase, source.job?.phase].find((phase) => phase !== undefined && phase !== '') ?? '';
  const failed = job?.state === 'failed';
  return <Dialog open size="md" title={t('source.prepare.detailsTitle')} onOpenChange={(open) => { if (!open) onClose(); }}
    footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    {read.isError ? <ErrorState compact error={read.error} retrying={read.isRefetching} onRetry={() => void read.refetch()} /> : job === undefined
      ? <SkeletonRows read={read} rows={2} /> : <div className="flex min-w-0 flex-col gap-3" data-source-job={job.state}>
        <Meter look="progress" value={job.progress.percent} label={t(`source.job.${job.state}`)} />
        <JobActivityLine job={job} />
        {job.error !== null ? <>
          <ErrorAlert error={job.error} message={t('source.step.line', { step: stepName(t, step), why: sourceFailure(t, job.error.code) })} />
          <p className="wrap-anywhere whitespace-pre-wrap text-sm" data-user-content>{job.error.message}</p>
        </> : failed && failedWhy !== null ? <p role="status" className="wrap-anywhere text-sm" data-source-failed-step={step === '' ? undefined : step}>
          {t('source.step.line', { step: stepName(t, step), why: failedWhy })}
        </p> : isOver(job.state) && <p role="status" className="text-sm">{t(`source.job.${job.state}`)}</p>}
      </div>}
  </Dialog>;
}
