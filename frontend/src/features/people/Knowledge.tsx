/** Current shared material and Read first paths, with no document bodies inserted into agent context. */
import { useQuery } from '@tanstack/react-query';
import { BookOpen, ChevronDown, ChevronRight, Diamond, FileText, Folder, LayoutGrid, MessageSquareText, MessagesSquare } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { agentSharesQuery, sharedTreeQuery, threadSharedQuery } from '@/api/share';
import { DocumentPolicy } from '@/features/people/DocumentPolicy';
import { AREAS, stepOf } from '@/features/access/agentShares';
import { AREA_WORDS, stepWords } from '@/features/access/AgentRow';
import { ErrorState } from '@/ui/ErrorState';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import type { AgentShare as PriorityShare, ThreadSharedGetReply as SharedView } from '@/generated/api-types';

/** Short context lines carry at most the chosen five paths; the full tree is read on demand. */
const CONTEXT_PATHS = 5;
function TreePage({ threadId, path }: { threadId: string; path: string }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const query = useQuery({ ...sharedTreeQuery(threadId, path), retry: false, staleTime: 0 });
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const entries = query.data.entries.filter((entry) => !entry.redacted && entry.name !== null);
  if (entries.length === 0) return <EmptyState compact icon={<BookOpen aria-hidden />} title={t('people.noMaterial')} />;
  return <ul className="flex flex-col gap-1">{entries.map((entry) => { const Icon = entry.kind === 'org' ? LayoutGrid : entry.kind === 'project' || entry.kind === 'folder' ? Folder : entry.kind === 'topic' ? Diamond : entry.kind === 'thread' || entry.kind === 'conversation' ? MessageSquareText : entry.kind === 'chat' ? MessagesSquare : FileText; return <li key={`${entry.id}:${entry.path}`} className="flex flex-col gap-1 border-l border-border pl-2">
    <span className="flex flex-wrap items-center gap-2 text-sm">{entry.kind === 'thread' || entry.kind === 'conversation' ? <ThreadStateIcon display={entry.thread_display} className="[&_svg]:size-4" /> : <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />}<span className="break-words">{entry.name}</span><span className="text-xs text-text-muted">{entry.writable ? t('access.choice.edit') : t('access.choice.view')}</span>
      {entry.parts !== null && <span className="text-xs text-text-muted">{entry.unit === 'block' ? t('people.blocksCount', { count: entry.parts }) : `${entry.parts} ${entry.unit ?? ''}`}</span>}
      {(entry.children ?? 0) > 0 && <Button size="sm" variant="ghost" icon={expanded.has(entry.path) ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />} aria-expanded={expanded.has(entry.path)} onClick={() => setExpanded((before) => new Set(before.has(entry.path) ? [...before].filter((one) => one !== entry.path) : [...before, entry.path]))}>{t(expanded.has(entry.path) ? 'people.closeMaterial' : 'people.openMaterial')}</Button>}
    </span>
    {expanded.has(entry.path) && <TreePage threadId={threadId} path={entry.path} />}
  </li>; })}</ul>;
}

export function ThreadKnowledge({ threadId, topicId }: { threadId: string; topicId: string }) {
  const { t } = useTranslation();
  const query = useQuery({ ...threadSharedQuery(threadId), retry: false, staleTime: 0 });
  const [all, setAll] = useState(false);
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const shared: SharedView = query.data;
  const shares: PriorityShare[] = shared.shares;
  const priority = [...new Map(shared.read_first_paths.map((path) => [path.id, path])).values()];
  return <div className="flex h-full flex-col gap-3 overflow-y-auto p-3"><DocumentPolicy topicId={topicId} />{shares.length === 0 ? <EmptyState compact title={t('people.noExplicitShares')} /> : <ShareList shares={shares} />}
    {priority.length > 0 && <section aria-label={t('people.readFirst')}><h3 className="text-sm font-semibold">{t('people.readFirst')}</h3><p className="text-xs text-text-muted">{t('people.readFirstHint')}</p><ul className="mt-1 text-sm">{priority.slice(0, CONTEXT_PATHS).map((entry) => <li key={entry.id} className="break-all font-mono text-xs">{entry.path}</li>)}</ul>{priority.length > CONTEXT_PATHS && <p className="text-xs text-text-muted">{t('people.moreMaterial')}</p>}</section>}
    <Button size="sm" icon={<BookOpen aria-hidden />} onClick={() => setAll(true)}>{t('people.listAll')}</Button>{all && <TreePage threadId={threadId} path="/" />}
  </div>;
}

function ShareList({ shares }: { shares: PriorityShare[] }) {
  const { t } = useTranslation();
  return <ul className="flex flex-col gap-2">{shares.map((share) => <li key={share.id} className="rounded-md border border-border p-2 text-sm"><span className="font-medium">{'title' in share.what ? share.what.title : 'name' in share.what ? share.what.name : ''}</span><span className="ml-2 text-xs text-text-muted">{share.by.display_name}</span><div className="mt-1 flex flex-wrap gap-2 text-xs text-text-muted">{AREAS.filter((area) => stepOf(area, share.parts) > 0).map((area) => <span key={area}>{t(AREA_WORDS[area])}: {stepWords(t, area, stepOf(area, share.parts))}</span>)}{share.read_first && <span>{t('people.readFirst')}</span>}</div></li>)}</ul>;
}
export function TopicKnowledge({ topicId }: { topicId: string }) {
  const { t } = useTranslation();
  const query = useQuery({ ...agentSharesQuery({ kind: 'topic', id: topicId }, 'to'), retry: false, staleTime: 0 });
  return <div className="flex flex-col gap-4"><DocumentPolicy topicId={topicId} />{query.isPending ? <SkeletonRows rows={2} /> : query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.data.length === 0 ? <EmptyState compact title={t('people.noExplicitShares')} /> : <ShareList shares={query.data} />}</div>;
}
