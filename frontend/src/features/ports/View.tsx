/** The Ports workbench entry, registered only from the environment permission served beside a thread. */
import { useQuery } from '@tanstack/react-query';
import { ACTIONS } from '@/ui/actions';
import { useEffect } from 'react';

import { environmentQuery } from '@/api/environments';
import { portsBrowser, showBrowserBeside } from '@/features/browser/View';
import { useRouteThread, useThreadDoor } from '@/features/files/door';
import { registerView } from '@/features/workbench/views';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { PortsPanel } from './Ports';

export function PortsView() {
  const state = useThreadDoor(useRouteThread());
  const id = state.kind === 'ready' ? state.access.environment.id : null;
  const allowed = state.kind === 'ready' && state.access.thread.environment_permissions.includes('ports.view');
  const query = useQuery({ ...environmentQuery(id ?? ''), enabled: id !== null && allowed });
  if (!allowed) return null;
  if (query.isPending) return <SkeletonRows rows={4} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  return <div className="h-full overflow-auto"><PortsPanel environment={query.data} browser={portsBrowser(query.data, showBrowserBeside)} /></div>;
}

/** Removing the served door removes its entry without granting anything from Thread View. */
export function usePortsRegistration(): void {
  useEffect(() => registerView({ name: 'ports', title: 'ports.title', about: 'ports.about', icon: ACTIONS.ports.Icon,
    screens: ['thread'], area: 'right', container: 'environment', order: 3, body: PortsView }), []);
}
