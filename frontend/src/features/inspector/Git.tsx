/** Read-only Git status and diffs.
 *
 * The state of the repository at the root of the workspace as its environment reports it (it reads that one
 * and no other): the branch, how far it is ahead of and behind where it follows, and what is uncommitted, in the three groups Git keeps
 * (staged, changed and not yet added). A row opens its difference beneath it; the menu opens it as a tab.
 * Where there is nothing to show it says why: not a repository, Git not installed, or the environment away.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, ChevronRight, ExternalLink, FileDiff, GitBranch, Link2, MoreHorizontal, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { gitDiff, gitStatusQuery, partOfDiff, workspaceKeys } from '@/api/workspace';
import { ApiError } from '@/api/client';
import { openDifference, openFile } from '@/features/files/FileTabs';
import { Publishing } from '@/features/source/Publish';
import type { WorkspaceAccess } from '@/features/files/door';
import type { GitChange } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { revealView } from '@/features/workbench/openView';
import { useViewShown } from '@/features/workbench/viewOpening';
import { cx } from '@/ui/cx';
import { DiffView } from '@/ui/DiffView';
import { EmptyState } from '@/ui/EmptyState';
import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { copyText } from '@/ui/clipboard';

import { PathLabel, ThreadViewFrame } from './parts';
/** Whether a bounded git read exceeded its time or output budget, as the machine explained it. */
export function gitReadTooSlow(error: unknown): boolean {
  if (error instanceof ApiError && (error.problem.reason === 'timeout' || error.problem.reason === 'too_large')) return true;
  const detail = error instanceof ApiError ? error.problem.detail : error instanceof Error ? error.message : '';
  return /git/i.test(detail) && /too slow|too large|timed? out|time(?:out| limit)/i.test(detail);
}

function GitLimit({ cut = false }: { cut?: boolean }) {
  const { t } = useTranslation();
  return <InlineAlert tone="warning" title={t(cut ? 'largeTree.gitCut' : 'largeTree.gitSlow')}
    action={<Button size="sm" onClick={() => revealView('terminal')}>{t('largeTree.openShell')}</Button>} />;
}

type Group = 'staged' | 'changed' | 'untracked';

/** The one letter an untracked file stands for in a column of one. */
const UNTRACKED_MARK = 'U';

/** The groups a change stands in: a file can be in two (staged, and changed again since). */
function groupsOf(change: GitChange): Group[] {
  if (change.index === '?' && change.worktree === '?') return ['untracked'];
  const groups: Group[] = [];
  if (change.index !== ' ' && change.index !== '?' && change.index !== '!') groups.push('staged');
  if (change.worktree !== ' ' && change.worktree !== '?' && change.worktree !== '!') groups.push('changed');
  return groups;
}

const LETTERS: Record<string, 'modified' | 'added' | 'deleted' | 'renamed' | 'copied' | 'conflict' | 'untracked'> = {
  M: 'modified',
  A: 'added',
  D: 'deleted',
  R: 'renamed',
  C: 'copied',
  U: 'conflict',
  '?': 'untracked',
};

function GitRow({ change, group, access, open, onToggle }: { change: GitChange; group: Group; access: WorkspaceAccess; open: boolean; onToggle: () => void }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const letter = group === 'staged' ? change.index : group === 'changed' ? change.worktree : '?';
  const word = LETTERS[letter] ?? 'modified';
  /** The letter Git's own short status writes: an untracked file is `??`, which a one-letter column writes as a U. */
  const mark = letter === '?' ? UNTRACKED_MARK : letter;
  const staged = group === 'staged';
  // The server hands over the difference of the whole repository, which every row of a group shares; a row cuts its file out of it.
  const diff = useQuery({
    queryKey: workspaceKeys.gitDiff(access.door, staged),
    queryFn: ({ signal }) => gitDiff(access.door, staged, signal),
    select: (whole) => ({ text: partOfDiff(whole.text, change.path), truncated: whole.truncated }),
    enabled: shown && open && group !== 'untracked',
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  });
  return (
    <li className="border-b border-border last:border-b-0" data-testid="git-row">
      <div className="flex items-center gap-1 pr-1">
        <button type="button" aria-expanded={open} onClick={onToggle} className="focus-inset flex min-h-(--row-height) min-w-0 flex-1 items-center gap-2 px-2 text-left text-base hover:bg-bg-muted">
          <ChevronRight className={cx('shrink-0 text-text-subtle transition-transform', open && 'rotate-90')} aria-hidden />
          <span className="w-4 shrink-0 text-center font-mono text-xs text-text-muted" title={t(`inspector.git.status.${word}`)}>
            {mark}
          </span>
          <PathLabel path={change.path} />
          <span className="sr-only">{t(`inspector.git.status.${word}`)}</span>
        </button>
        <Menu
          label={t('inspector.changes.menuFor', { path: change.path })}
          trigger={
            <button type="button" aria-label={t('inspector.changes.menuFor', { path: change.path })} className="focus-inset inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-border hover:text-text">
              <MoreHorizontal aria-hidden />
            </button>
          }
          entries={[
            {
              id: 'diff',
              label: t('inspector.changes.openDifference'),
              icon: <FileDiff aria-hidden />,
              disabled: group === 'untracked' ? t('inspector.git.noDifference') : false,
              onSelect: () => openDifference({ kind: 'git', environmentId: access.door.environmentId, threadId: access.thread.id, path: change.path, staged }),
            },
            {
              id: 'file',
              label: t('inspector.changes.openFile'),
              icon: <ExternalLink aria-hidden />,
              disabled: word === 'deleted' ? t('inspector.changes.deleted') : false,
              onSelect: () => openFile({ environmentId: access.door.environmentId, threadId: access.thread.id, path: change.path, permanent: false }),
            },
            { id: 'sep', type: 'separator' },
            { id: 'copy', label: t('files.menu.copyPath'), icon: <Link2 aria-hidden />, onSelect: () => void copyText(change.path) },
          ]}
        />
      </div>
      {open && (
        <div className="px-2 pb-2">
          {group === 'untracked' ? (
            <p className="px-1 text-sm text-text-muted">{t('inspector.git.untracked')}</p>
          ) : diff.error !== null ? (
            gitReadTooSlow(diff.error) ? <GitLimit /> : <ErrorState compact error={diff.error} onRetry={() => void diff.refetch()} />
          ) : diff.data === undefined ? (
            <SkeletonRows rows={3} />
          ) : (
            <><DiffView diff={diff.data.text === '' ? null : diff.data.text} truncated={diff.data.truncated} label={t('files.difference.label', { name: change.path })} />{diff.data.truncated && <GitLimit cut />}</>
          )}
        </div>
      )}
    </li>
  );
}

function GitPanel({ access }: { access: WorkspaceAccess }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const client = useQueryClient();
  const [open, setOpen] = useState<string | null>(null);
  const query = useQuery({ ...gitStatusQuery(access.door), enabled: shown, staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false, refetchOnReconnect: false, retry: false });
  const status = query.data;
  if (query.error !== null && status === undefined) return gitReadTooSlow(query.error) ? <GitLimit /> : <ErrorState compact className="h-full" error={query.error} onRetry={() => void query.refetch()} />;
  if (status === undefined) return <SkeletonRows rows={5} />;
  if (!status.available) {
    const reason = status.reason ?? 'not_a_repository';
    return (
      <EmptyState
        compact
        className="h-full"
        icon={<GitBranch aria-hidden />}
        title={reason === 'git_missing' ? t('inspector.git.missing') : reason === 'offline' ? t('inspector.git.offline') : t('inspector.git.needsRepository')}
      />
    );
  }
  const groups: { group: Group; changes: GitChange[] }[] = (['staged', 'changed', 'untracked'] as const).map((group) => ({ group, changes: status.changes.filter((change) => groupsOf(change).includes(group)) }));
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="git-panel">
      <div className="flex items-center gap-2 border-b border-border px-3 py-2 text-sm">
        <GitBranch className="shrink-0 text-text-muted" aria-hidden />
        <span className="min-w-0 truncate font-medium text-text" data-testid="git-branch">
          {status.branch ?? t('inspector.git.detached')}
        </span>
        {status.ahead !== null && status.ahead > 0 && (
          <span className="inline-flex items-center gap-1 text-xs text-text-muted tabular-nums" title={t('inspector.git.ahead', { count: status.ahead })}>
            <ArrowUp aria-hidden />
            {status.ahead}
            <span className="sr-only">{t('inspector.git.ahead', { count: status.ahead })}</span>
          </span>
        )}
        {status.behind !== null && status.behind > 0 && (
          <span className="inline-flex items-center gap-1 text-xs text-text-muted tabular-nums" title={t('inspector.git.behind', { count: status.behind })}>
            <ArrowDown aria-hidden />
            {status.behind}
            <span className="sr-only">{t('inspector.git.behind', { count: status.behind })}</span>
          </span>
        )}
        <span className="flex-1" />
        {status.head !== null && <span className="font-mono text-xs text-text-muted">{status.head.slice(0, 7)}</span>}
        <IconButton size="sm" label={t('inspector.git.refresh')} icon={<RefreshCw aria-hidden />} onClick={() => void client.invalidateQueries({ queryKey: ['workspace-git', access.door.environmentId] })} />
      </div>
      <Publishing access={access} status={status} />
      {status.changes.length === 0 ? (
        <EmptyState compact className="flex-1" icon={<GitBranch aria-hidden />} title={t('inspector.git.clean')} />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          {groups
            .filter((entry) => entry.changes.length > 0)
            .map((entry) => (
              <section key={entry.group} aria-label={t(`inspector.git.group.${entry.group}`)}>
                <h3 className="bg-bg-subtle px-3 py-1 text-xs font-medium text-text-muted">{t(`inspector.git.group.${entry.group}`)} ({entry.changes.length})</h3>
                <ul>
                  {entry.changes.map((change) => {
                    const key = `${entry.group}.${change.path}`;
                    return <GitRow key={key} change={change} group={entry.group} access={access} open={open === key} onToggle={() => setOpen(open === key ? null : key)} />;
                  })}
                </ul>
              </section>
            ))}
        </div>
      )}
    </div>
  );
}

/** The Git view of a thread. */
export function GitView(_props: ViewProps) {
  return <ThreadViewFrame need="files.view" icon={<GitBranch aria-hidden />}>{(access) => <GitPanel access={access} />}</ThreadViewFrame>;
}
