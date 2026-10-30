/** The body of the VNC view beside a thread: the displays of the thread's environment, as far as the reader may watch them. */
import { useQuery } from '@tanstack/react-query';
import { MonitorPlay } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { environmentQuery } from '@/api/environments';
import { useRouteThread, useThreadDoor } from '@/features/files/door';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

import { DisplaysPanel } from './Displays';

export function VncView() {
  const { t } = useTranslation();
  const threadId = useRouteThread();
  const state = useThreadDoor(threadId);
  const id = state.kind === 'ready' ? state.access.environment.id : null;
  const allowed = state.kind === 'ready' && state.access.thread.environment_permissions.includes('vnc.view');
  const query = useQuery({ ...environmentQuery(id ?? ''), enabled: id !== null && allowed });
  if (state.kind === 'loading') return <SkeletonRows rows={4} />;
  if (!allowed) return <EmptyState compact className="h-full" icon={<MonitorPlay aria-hidden />} title={t('vnc.view.notAllowed')} />;
  if (query.isPending) return <SkeletonRows rows={4} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  return <DisplaysPanel environment={query.data} threadId={threadId} />;
}
