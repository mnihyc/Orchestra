/** Where a thread's files come from, while they are being made ready: the repository, branch and directory from the server's facts, the job's progress, and why a preparation failed, with Prepare again.
 *
 * A thread whose source is a repository or a worktree on a new branch holds its first message until the
 * source is ready; this says so above the conversation. A directory that was there already is kept
 * on every machine. Nothing here guesses a branch or an address.
 */
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Folder, FolderGit2 } from 'lucide-react';
import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { prepareSource } from '@/api/source';
import { workspaceDenial } from '@/features/files/door';
import type { SourceInfo, Thread } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Meter } from '@/ui/Meter';
import { sourceFailure } from './words';

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
  const environment = thread.environment;
  const again = useMutation({ mutationFn: () => prepareSource(environment?.id ?? '', thread.id, key.current) });
  const source = thread.source;
  if (source === null || source.state === 'ready' || environment === null || thread.state === 'blocked' && thread.blocked?.reason === 'environment_unavailable') return null;
  const mayPrepare = thread.environment_permissions.includes(PREPARE_PERMISSION);
  if (source.state === 'failed') {
    return (
      <InlineAlert
        tone="danger"
        title={t('source.prepare.failed')}
        action={
          mayPrepare ? (
            <Button action="retry" size="sm" pending={again.isPending} onClick={() => again.mutate()}>
              {t('source.prepare.again')}
            </Button>
          ) : undefined
        }
      >
        <span className="flex flex-col gap-1" data-source-state="failed">
          <span className="wrap-anywhere">{sourceLine(t, source)}</span>
          {source.reason !== null && <span className="wrap-anywhere">{sourceFailure(t, source.reason)}</span>}
          <span>{t('source.prepare.held')}</span>
          {!mayPrepare && <span className="text-text-muted">{workspaceDenial(t, PREPARE_PERMISSION, environment)}</span>}
          {again.error !== null && <ErrorAlert error={again.error} />}
        </span>
      </InlineAlert>
    );
  }
  return (
    <InlineAlert tone="info" title={source.state === 'pending' ? t('source.prepare.pending') : t('source.prepare.preparing')}>
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
  );
}
