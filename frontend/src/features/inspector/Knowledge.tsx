/** What this thread's agent can read and write beyond its own thread, as the tree its tools show: a pen where it may write, a plus where it may make documents.
 * Reads the agent's own tree a branch at a time; redacted entries never expose a name or a path.
 */
import { useQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, FileText, Folder, Lock, Pencil, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { sharedTreeQuery, threadSharedQuery } from '@/api/share';
import { DocumentPolicy } from '@/features/people/DocumentPolicy';
import { IsolationBadge } from '@/features/thread/BindingSummary';
import type { SharedEntry, Thread } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';
import { ThreadPanel } from './ThreadPanel';
import { ShareToAgent } from './ShareToAgent';

function Entry({ entry, threadId }: { entry: SharedEntry; threadId: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (entry.redacted) return <li className="flex items-center gap-2 px-2 py-1 text-sm text-text-subtle"><Lock aria-hidden />{t('threadViews.knowledge.hidden')}</li>;
  const folder = entry.children !== null;
  return <li className="py-1">
    <div className="flex flex-wrap items-center gap-1 text-sm">
      {folder ? <button type="button" className="focus-ring flex min-w-0 items-center gap-1 rounded-md" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}<Folder aria-hidden /><span className="wrap-anywhere">{entry.name}</span></button> : <span className="flex items-center gap-1 wrap-anywhere"><FileText aria-hidden />{entry.name}</span>}
      {entry.writable && <span role="img" aria-label={t('threadViews.knowledge.write')} title={t('threadViews.knowledge.write')}><Pencil size={16} aria-hidden /></span>}
      {folder && entry.writable && <span role="img" aria-label={t('threadViews.knowledge.create')} title={t('threadViews.knowledge.create')}><Plus size={16} aria-hidden /></span>}
      {entry.shared_by !== null && <span className="flex items-center gap-1 text-xs text-text-muted">{t('threadViews.knowledge.by')} <UserChip user={entry.shared_by} size={24} /></span>}
    </div>
    {open && folder && <div className="ml-3 border-l border-border pl-2"><Branch threadId={threadId} path={entry.path} /></div>}
  </li>;
}

function Branch({ threadId, path = '/' }: { threadId: string; path?: string }) {
  const { t } = useTranslation();
  const query = useQuery(sharedTreeQuery(threadId, path));
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={3} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  return <><ul aria-label={t('threadViews.knowledge.tree')}>{query.data.entries.map((entry, index) => <Entry key={`${entry.id}:${index}`} entry={entry} threadId={threadId} />)}</ul>
    {query.data.entries.length === 0 && <EmptyState compact icon={<Folder aria-hidden />} title={t('threadViews.knowledge.empty')} />}
    {query.data.truncated && <p className="text-sm text-text-muted">{t('threadViews.knowledge.truncated')}</p>}</>;
}

function Knowledge({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const shares = useQuery(threadSharedQuery(thread.id));
  const [sharing, setSharing] = useState(false);
  return <div className="h-full overflow-auto p-3" data-testid="thread-knowledge">
    <DocumentPolicy topicId={thread.parent_id ?? thread.id} />
    {thread.binding?.isolation.orch_tools.offered === false && <div className="pb-3"><IsolationBadge thread={thread} /></div>}
    <Button size="sm" onClick={() => setSharing(true)}>{t('threadViews.knowledge.share')}</Button>
    {shares.error !== null && <ErrorState compact error={shares.error} onRetry={() => void shares.refetch()} />}
    {shares.data !== undefined && <><p className="py-3 text-sm text-text-muted">{t(`threadViews.knowledge.policy.${shares.data.policy}`)}</p>
      {shares.data.read_first_paths.length > 0 && <section aria-label={t('people.readFirst')} className="pb-3"><h3 className="text-sm font-semibold">{t('people.readFirst')}</h3><p className="text-xs text-text-muted">{t('people.readFirstHint')}</p><ul>{shares.data.read_first_paths.slice(0, 5).map((material) => <li key={material.id} className="break-all font-mono text-xs">{material.path}</li>)}</ul>{shares.data.read_first_paths.length > 5 && <p className="text-xs text-text-muted">{t('people.moreMaterial')}</p>}</section>}
      {shares.data.shares.filter((share) => share.mode === 'snapshot').map((share) => <p key={share.id} className="pb-2 text-sm">{t('threadViews.knowledge.copy', { name: share.by.display_name })}</p>)}</>}
    <Branch threadId={thread.id} />
    {sharing && <ShareToAgent thread={thread} onClose={() => setSharing(false)} />}
  </div>;
}

export function KnowledgeView(_props: ViewProps) { return <ThreadPanel>{(thread) => <Knowledge key={thread.id} thread={thread} />}</ThreadPanel>; }
