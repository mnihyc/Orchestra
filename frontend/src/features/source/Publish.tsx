/** Commit and Push in the Git view: the files chosen, the head and status they were read at, the caller as author, one job each, and the pull request page only after a push succeeded.
 *
 * A commit lists the changed paths of the repository to choose from, needs a message, and says who it is
 * written as; paths somebody else staged stay staged. A push names the remote, the branch and whether the
 * branch is to follow it from then on; it never forces and never pulls first. Each is one job, followed in
 * the view and never asked for twice after a lost answer; a failure shows Git's own safe words. What the
 * server refuses (no right, no remote write here, a status that changed since it was read) is said where
 * the form is. The pull request page comes from the push's answer; an address Orch could not make is
 * left to the remote's own page.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowUpFromLine, ExternalLink, GitCommitHorizontal } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { meQuery } from '@/api/auth';
import { isOver, jobQuery } from '@/api/forks';
import { commitFiles, pushBranch } from '@/api/source';
import { workspaceKeys } from '@/api/workspace';
import { type WorkspaceAccess, workspaceDenial } from '@/features/files/door';
import type { GitStatus, Job } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Meter } from '@/ui/Meter';
import { JobActivityLine } from '@/features/change/Progress';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Textarea } from '@/ui/Textarea';
import { Tooltip } from '@/ui/Tooltip';

import { pullRequestUrl } from './words';

/** The two acts of publishing, under the Git view's heading, for whoever may write the environment's files. */
export function Publishing({ access, status }: { access: WorkspaceAccess; status: GitStatus }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<'commit' | 'push' | null>(null);
  const denied = access.mayManage ? null : workspaceDenial(t, 'files.manage', access.environment);
  const commitWhy = denied ?? (status.changes.length === 0 ? t('source.git.nothingToCommit') : status.status_hash === null ? t('source.git.statusUnknown') : null);
  const pushWhy = denied ?? (status.remotes.length === 0 ? t('source.git.noRemote') : status.head === null || status.branch === null ? t('source.git.noBranch') : null);
  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2" data-git-publishing>
      <Tooltip content={commitWhy ?? t('source.git.commitHint')}>
        <Button size="sm" icon={<GitCommitHorizontal aria-hidden />} blocked={commitWhy !== null} onClick={() => setOpen('commit')}>
          {t('source.git.commit')}
        </Button>
      </Tooltip>
      <Tooltip content={pushWhy ?? t('source.git.pushHint')}>
        <Button size="sm" icon={<ArrowUpFromLine aria-hidden />} blocked={pushWhy !== null} onClick={() => setOpen('push')}>
          {t('source.git.push')}
        </Button>
      </Tooltip>
      {open === 'commit' && <CommitDialog access={access} status={status} onClose={() => setOpen(null)} />}
      {open === 'push' && <PushDialog access={access} status={status} onClose={() => setOpen(null)} />}
    </div>
  );
}

/** A job of the view, as far as it has come: its bar, then its end in plain words. */
function JobLine({ job, label }: { job: Job | undefined; label: string }) {
  const { t } = useTranslation();
  if (job === undefined) return <p role="status" className="text-sm text-text-muted">{t('source.job.queued')}</p>;
  return (
    <div className="flex flex-col gap-2" data-git-job={job.state}>
      {job.error === null && isOver(job.state) && <p role="status" className="text-sm">{t(`source.job.${job.state}`)}</p>}
      {!isOver(job.state) && <><Meter look="progress" value={job.progress.percent} label={label} /><JobActivityLine job={job} /></>}
      {job.error !== null && <ErrorAlert error={job.error} />}
    </div>
  );
}

/** Follows one accepted job, and reads the repository again once it has ended. */
function useGitJob(access: WorkspaceAccess, id: string | null) {
  const client = useQueryClient();
  const job = useQuery({ ...jobQuery(id ?? ''), enabled: id !== null });
  const ended = job.data !== undefined && isOver(job.data.state) ? job.data.id : null;
  const environmentId = access.door.environmentId;
  useEffect(() => {
    if (ended !== null) void client.invalidateQueries({ queryKey: workspaceKeys.git({ environmentId, threadId: null }) });
  }, [client, ended, environmentId]);
  return job;
}

function CommitDialog({ access, status, onClose }: { access: WorkspaceAccess; status: GitStatus; onClose: () => void }) {
  const { t } = useTranslation();
  const me = useQuery(meQuery);
  const paths = [...new Set(status.changes.map((change) => change.path))];
  const [chosen, setChosen] = useState<ReadonlySet<string>>(() => new Set(paths));
  const [message, setMessage] = useState('');
  const [jobId, setJobId] = useState<string | null>(null);
  const key = useRef(newActKey());
  const commit = useMutation({
    mutationFn: () => commitFiles(access.door, { path: status.repository_dir ?? '', paths: paths.filter((path) => chosen.has(path)), message: message.trim(), expected_head: status.head, expected_status: status.status_hash ?? '' }, key.current),
    onSuccess: (job) => setJobId(job.id),
  });
  const job = useGitJob(access, jobId);
  const running = jobId !== null && (job.data === undefined || !isOver(job.data.state));
  const done = job.data?.state === 'succeeded';
  // A changed choice is another act: a press after it is not the repeat of the one before.
  const changed = () => {
    key.current = newActKey();
    commit.reset();
  };
  const toggle = (path: string, on: boolean) => {
    changed();
    setChosen((before) => {
      const next = new Set(before);
      if (on) next.add(path);
      else next.delete(path);
      return next;
    });
  };
  const ready = chosen.size > 0 && message.trim() !== '' && status.status_hash !== null;
  return (
    <Dialog
      open
      size="md"
      title={t('source.git.commitTitle')}
      description={t('source.git.commitLead')}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{running ? t('source.restore.hide') : t('common.close')}</Button>
          {jobId === null && (
            <Button variant="primary" icon={<GitCommitHorizontal aria-hidden />} pending={commit.isPending} disabled={!ready} onClick={() => commit.mutate()}>
              {t('source.git.commitDo')}
            </Button>
          )}
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (ready && jobId === null) commit.mutate(); }}>
        <dl className="flex flex-wrap gap-x-4 gap-y-1 text-sm text-text-muted">
          <div className="flex gap-2"><dt>{t('source.git.branch')}</dt><dd>{status.branch ?? t('inspector.git.detached')}</dd></div>
          {status.head !== null && <div className="flex gap-2"><dt>{t('source.git.revision')}</dt><dd><code>{status.head.slice(0, 7)}</code></dd></div>}
        </dl>
        <fieldset className="flex flex-col gap-1" disabled={jobId !== null}>
          <legend className="mb-1 text-sm font-medium">{t('source.git.files', { count: chosen.size })}</legend>
          <Checkbox label={t('source.git.allFiles')} checked={chosen.size === paths.length ? true : chosen.size === 0 ? false : 'indeterminate'} onCheckedChange={(on) => { changed(); setChosen(on ? new Set(paths) : new Set()); }} />
          <div className="flex max-h-56 flex-col gap-1 overflow-auto rounded-lg border border-border p-2">
            {paths.map((path) => (
              <Checkbox key={path} label={<code className="wrap-anywhere">{path}</code>} checked={chosen.has(path)} onCheckedChange={(on) => toggle(path, on)} />
            ))}
          </div>
        </fieldset>
        <p className="text-sm text-text-muted">{t('source.git.othersStaged')}</p>
        <Textarea label={t('source.git.message')} value={message} rows={4} disabled={jobId !== null} onChange={(event) => { changed(); setMessage(event.target.value); }} />
        <p className="text-sm" data-commit-author>
          {t('source.git.author', { name: me.data?.display_name ?? '…' })}
        </p>
        {commit.error !== null && <ErrorAlert error={commit.error} />}
        {jobId !== null && <JobLine job={job.data} label={t('source.git.commitTitle')} />}
        {done && <InlineAlert tone="success">{t('source.git.committed')}</InlineAlert>}
      </form>
    </Dialog>
  );
}

function PushDialog({ access, status, onClose }: { access: WorkspaceAccess; status: GitStatus; onClose: () => void }) {
  const { t } = useTranslation();
  const [remote, setRemote] = useState(status.remotes.find((one) => status.upstream?.startsWith(`${one.name}/`) === true)?.name ?? status.remotes[0]?.name ?? '');
  const [branch, setBranch] = useState(status.branch ?? '');
  const [upstream, setUpstream] = useState(status.upstream === null);
  const [jobId, setJobId] = useState<string | null>(null);
  const key = useRef(newActKey());
  const chosen = status.remotes.find((one) => one.name === remote);
  const push = useMutation({
    mutationFn: () => pushBranch(access.door, { path: status.repository_dir ?? '', remote, expected_remote_url: chosen?.url ?? '', branch: branch.trim(), expected_head: status.head ?? '', set_upstream: upstream }, key.current),
    onSuccess: (job) => setJobId(job.id),
  });
  const job = useGitJob(access, jobId);
  const running = jobId !== null && (job.data === undefined || !isOver(job.data.state));
  const page = job.data?.state === 'succeeded' ? pullRequestUrl(job.data.result) : null;
  const remotePage = chosen !== undefined && /^https?:\/\//.test(chosen.url) ? chosen.url : null;
  const changed = () => {
    key.current = newActKey();
    push.reset();
  };
  const ready = chosen !== undefined && branch.trim() !== '' && status.head !== null;
  return (
    <Dialog
      open
      size="md"
      title={t('source.git.pushTitle')}
      description={t('source.git.pushLead')}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{running ? t('source.restore.hide') : t('common.close')}</Button>
          {jobId === null && (
            <Button variant="primary" icon={<ArrowUpFromLine aria-hidden />} pending={push.isPending} disabled={!ready} onClick={() => push.mutate()}>
              {t('source.git.pushDo')}
            </Button>
          )}
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (ready && jobId === null) push.mutate(); }}>
        <ChoiceLocator<string>
          label={t('source.git.remote')}
          value={remote}
          disabled={jobId !== null}
          onValueChange={(value) => { changed(); setRemote(value); }}
          options={status.remotes.map((one) => ({ value: one.name, label: one.name, description: one.url }))}
        />
        <Input label={t('source.git.branch')} value={branch} mono disabled={jobId !== null} onChange={(event) => { changed(); setBranch(event.target.value); }} />
        <Checkbox label={t('source.git.setUpstream')} hint={status.upstream === null ? t('source.git.noUpstream') : t('source.git.hasUpstream', { upstream: status.upstream })} checked={upstream} disabled={jobId !== null} onCheckedChange={(on) => { changed(); setUpstream(on); }} />
        <p className="text-sm text-text-muted">{t('source.git.neverForce')}</p>
        {push.error !== null && <ErrorAlert error={push.error} />}
        {jobId !== null && <JobLine job={job.data} label={t('source.git.pushTitle')} />}
        {job.data?.state === 'succeeded' && (
          <InlineAlert tone="success" title={t('source.git.pushed')}>
            {page !== null ? (
              <Button asChild size="sm" icon={<ExternalLink aria-hidden />}>
                <a href={page} target="_blank" rel="noopener noreferrer">{t('source.git.openPullRequest')}</a>
              </Button>
            ) : remotePage !== null ? (
              <span className="flex flex-col gap-1">
                <span>{t('source.git.unknownForge')}</span>
                <Button asChild size="sm" icon={<ExternalLink aria-hidden />}>
                  <a href={remotePage} target="_blank" rel="noopener noreferrer">{t('source.git.openRemote')}</a>
                </Button>
              </span>
            ) : (
              t('source.git.unknownForge')
            )}
          </InlineAlert>
        )}
      </form>
    </Dialog>
  );
}
