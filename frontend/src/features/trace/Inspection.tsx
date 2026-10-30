import { threadIconFacts } from '@/ui/ThreadState';
/** Read-only retained metadata, conversation and blocks through the existing stored readers. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { ArrowLeft, Eye, FileText, Folder, MessageSquareText } from 'lucide-react';
import { lazy, Suspense } from 'react';
import { useTranslation } from 'react-i18next';
import { traceJson } from '@/api/trace';
import type { ThreadItemListReply, DocumentSync as InspectedDocument, Node as InspectedNode, ThreadSync as InspectedThread } from '@/generated/api-types';
import { StoredTimeline } from '@/features/timeline/Timeline';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { RetainedEnvironmentInspection } from './KeptEnvironments';
import { InspectionBanner } from './InspectionBanner';
export { InspectionBanner } from './InspectionBanner';
import { inspectionHref } from './objects';

const DocumentFolderPage = lazy(async () => ({ default: (await import('@/features/documents/DirectoryBrowser')).DocumentFolderPage }));
const BlockEditor = lazy(async () => ({ default: (await import('@/features/documents/Editor')).BlockEditor }));
const STORED_PAGE = 200;
function Conversation({ id }: { id: string }) {
  const { t } = useTranslation();
  const read = useQuery({ queryKey: ['deleted-inspection', 'thread', id], queryFn: ({ signal }) => traceJson<InspectedThread>(`/threads/${id}/sync?inspect_deleted=true&limit=${STORED_PAGE}`, { signal }), staleTime: 0, gcTime: 0 });
  const older = useInfiniteQuery({ queryKey: ['deleted-inspection', 'items', id], initialPageParam: null as number | null, queryFn: ({ pageParam, signal }) => traceJson<ThreadItemListReply>(`/threads/${id}/items?inspect_deleted=true&before=${pageParam ?? read.data?.items[0]?.seq_created ?? 0}&limit=${STORED_PAGE}`, { signal }), getNextPageParam: (page) => page.has_older ? page.items[0]?.seq_created : undefined, enabled: false, gcTime: 0 });
  useScreenHeading({ title: read.data?.thread.name ?? t('trace.inspect'), icon: MessageSquareText, thread: threadIconFacts(read.data?.thread), ...(read.data === undefined ? {} : { path: read.data.thread.path }) });
  if (read.isPending) return <SkeletonRows rows={4} />;
  if (read.isError) return <ErrorState error={read.error} title={t('trace.inspectFailure')} onRetry={() => void read.refetch()} />;
  const data = read.data; const pages = older.data?.pages ?? []; const snapshot = { ...data, items: [...pages.flatMap((page) => page.items), ...data.items].sort((a, b) => a.seq_created - b.seq_created) };
  const inspection = data.thread.inspection;
  if (inspection === undefined || inspection === null) return <EmptyState icon={<Eye aria-hidden />} title={t('trace.noContent')} />;
  return <><InspectionBanner facts={inspection} /><Pagination hasMore={pages.length === 0 ? data.has_older : older.hasNextPage} loading={older.isFetchingNextPage} error={older.error ?? undefined} onMore={() => void older.fetchNextPage()} /><StoredTimeline snapshot={snapshot} />{snapshot.items.length === 0 && <EmptyState compact icon={<MessageSquareText aria-hidden />} title={t('trace.nothingKept')} />}</>;
}
function Blocks({ id }: { id: string }) {
  const { t } = useTranslation(); const read = useQuery({ queryKey: ['deleted-inspection', 'document', id], queryFn: ({ signal }) => traceJson<InspectedDocument>(`/documents/${id}/sync?inspect_deleted=true`, { signal }), staleTime: 0, gcTime: 0 });
  useScreenHeading({ title: read.data?.document.title ?? t('trace.inspect'), icon: FileText, ...(read.data === undefined ? {} : { path: read.data.document.path, leaf: true }) });
  if (read.isPending) return <SkeletonRows rows={4} />;
  if (read.isError) return <ErrorState error={read.error} title={t('trace.inspectFailure')} onRetry={() => void read.refetch()} />;
  const inspection = read.data.document.inspection;
  if (inspection === undefined || inspection === null) return <EmptyState icon={<Eye aria-hidden />} title={t('trace.noContent')} />;
  return <><InspectionBanner facts={inspection} /><h2 className="text-xl font-semibold wrap-anywhere">{read.data.document.title}</h2>{read.data.blocks.length === 0 ? <EmptyState compact icon={<FileText aria-hidden />} title={t('trace.nothingKept')} /> : <Suspense fallback={<SkeletonRows rows={4} />}><BlockEditor document={read.data.document} editable={false} fixed={read.data.blocks} inspection /></Suspense>}</>;
}
function Place({ id }: { id: string }) {
  const { t } = useTranslation(); const read = useQuery({ queryKey: ['deleted-inspection', 'node', id], queryFn: ({ signal }) => traceJson<InspectedNode>(`/nodes/${id}?inspect_deleted=true`, { signal }), staleTime: 0, gcTime: 0 });
  const children = useInfiniteQuery({ queryKey: ['deleted-inspection', 'children', id], initialPageParam: null as string | null, queryFn: ({ pageParam, signal }) => traceJson<{ items: InspectedNode[]; next_cursor: string | null }>(`/nodes/${id}/children?inspect_deleted=true${pageParam === null ? '' : `&cursor=${pageParam}`}`, { signal }), getNextPageParam: (page) => page.next_cursor, enabled: read.isSuccess && read.data.kind !== 'thread' && Boolean(read.data.inspection), gcTime: 0 });
  useScreenHeading({ title: read.data?.name ?? t('trace.inspect'), icon: Eye, ...(read.data === undefined ? {} : { path: read.data.path }) });
  if (read.isPending) return <SkeletonRows rows={4} />;
  if (read.isError) return <ErrorState error={read.error} title={t('trace.inspectFailure')} onRetry={() => void read.refetch()} />;
  const inspection = read.data.inspection;
  if (inspection === undefined || inspection === null) return <EmptyState icon={<Eye aria-hidden />} title={t('trace.noContent')} />;
  const entries = children.data?.pages.flatMap((page) => page.items) ?? [];
  return <><InspectionBanner facts={inspection} /><h2 className="text-xl font-semibold wrap-anywhere">{read.data.name}</h2><p className="wrap-anywhere text-text-muted">{read.data.description}</p><Section id="inspected-children" title={t('trace.children')}>{children.isPending ? <SkeletonRows rows={3} /> : children.isError ? <ErrorState compact error={children.error} onRetry={() => void children.refetch()} /> : entries.length === 0 ? <EmptyState compact icon={<Folder aria-hidden />} title={t('trace.nothingKept')} /> : <ul className="divide-y divide-border rounded-lg border border-border">{entries.map((entry) => <li key={entry.id} className="flex flex-wrap items-center justify-between gap-2 p-3"><span className="wrap-anywhere">{entry.name}</span><Button size="sm" action="show" asChild><a href={inspectionHref(entry.kind, entry.id)}>{t('trace.inspect')}</a></Button></li>)}</ul>}<Pagination hasMore={children.hasNextPage} loading={children.isFetchingNextPage} error={children.error ?? undefined} onMore={() => void children.fetchNextPage()} /></Section></>;
}
export function InspectionPage({ kind, id }: { kind: string; id: string }) {
  const { t } = useTranslation();
  if (kind === 'document_container') return <Suspense fallback={<Page><SkeletonRows rows={4} /></Page>}><DocumentFolderPage id={id} retained /></Suspense>;
  const isPlace = kind === 'org' || kind === 'project' || kind === 'topic';
  return <Page><PageHeader mark={<PlaceMark>{kind === 'document' ? <FileText aria-hidden /> : <Eye aria-hidden />}</PlaceMark>} title={t('trace.inspect')} actions={<Button icon={<ArrowLeft aria-hidden />} onClick={() => window.history.back()}>{t('trace.back')}</Button>} />{kind === 'thread' ? <Conversation id={id} /> : kind === 'document' ? <Blocks id={id} /> : kind === 'environment' ? <RetainedEnvironmentInspection id={id} /> : isPlace ? <Place id={id} /> : <UnknownInspection />}</Page>;
}
function UnknownInspection() {
  const { t } = useTranslation(); useScreenHeading({ title: t('trace.inspect'), icon: Eye });
  return <EmptyState icon={<Eye aria-hidden />} title={t('trace.noContent')} />;
}
