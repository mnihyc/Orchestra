/** A thread view that works even when its thread has no environment. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { MessageSquareText } from 'lucide-react';
import { useRouteThread } from '@/features/files/door';
import type { Thread } from '@/generated/api-types';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useThreadFeed, reloadThread } from '@/realtime/live';
import { useThreadView } from '@/realtime/stores/thread';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function ThreadPanel({ manage = false, threadId, icon = <MessageSquareText aria-hidden />, children }: { manage?: boolean; threadId?: string | undefined; icon?: ReactNode; children: (thread: Thread) => ReactNode }) {
  const { t } = useTranslation();
  const route = useRouteThread();
  const id = threadId ?? route;
  const read = useThreadReadRef(id ?? '');
  useThreadFeed(id);
  const view = useThreadView(id ?? '');
  if (id === null) return <EmptyState compact icon={icon} title={t('inspector.frame.noThread')} />;
  if (view.closed !== null) return <EmptyState compact icon={icon} title={t('threadViews.unavailable')} />;
  if (view.error !== null && view.thread === null) return <ErrorState compact error={view.error} onRetry={() => reloadThread(read)} />;
  if (view.thread === null) return <SkeletonRows rows={4} />;
  if (manage && !view.thread.access.permissions.includes('thread.manage')) return <EmptyState compact icon={icon} title={t('threadViews.manageRequired')} />;
  return <>{children(view.thread)}</>;
}
