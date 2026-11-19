/** Browser and display lists open their existing viewer in the main area. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { AppWindow, MonitorPlay } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { browsersQuery, browserKeys } from '@/api/browsers';
import { displaysQuery, displayKeys } from '@/api/displays';
import { useBrowserRequests } from '@/features/browser/store';
import { useRouteThread, useThreadDoor } from '@/features/files/door';
import { revealView } from '@/features/workbench/openView';
import { useViewShown } from '@/features/workbench/viewOpening';
import { socket } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';

function useTargetsFeed(id: string | null, kind: 'browsers' | 'displays') {
  const client = useQueryClient();
  useEffect(() => {
    if (id === null) return;
    const subscription = socket.subscribe(`env:${id}:${kind}`, { since: () => null, onFrame: (frame) => {
      if (frame.op === 'event' || frame.op === 'sync') void client.invalidateQueries({ queryKey: kind === 'browsers' ? browserKeys.list(id) : displayKeys.list(id) });
    } });
    return () => subscription.unsubscribe();
  }, [client, id, kind]);
}

export function BrowsersSection() {
  const { t } = useTranslation();
  const shown = useViewShown();
  const state = useThreadDoor(useRouteThread());
  const id = state.kind === 'ready' && state.access.thread.environment_permissions.includes('browser.view') ? state.access.environment.id : null;
  const query = useQuery({ ...browsersQuery(id ?? ''), enabled: shown && id !== null });
  useTargetsFeed(shown ? id : null, 'browsers');
  if (id === null) return null;
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const browsers = query.data.browsers.filter((browser) => browser.state !== 'closed');
  return <div className="relative h-full min-h-0 overflow-y-auto p-2">
    {browsers.length === 0 && <EmptyState compact title={t('browser.list.empty')} icon={<AppWindow aria-hidden />} />}
    <ul aria-label={t('browser.list.label')}>
      {browsers.map((browser) => {
        const state = t(`browser.state.${browser.state}`);
        const name = t('workbench.targetNames.browser', { name: browser.profile ?? t('browser.list.ownProfile'), state });
        return <li key={browser.id}><Button variant="ghost" size="sm" aria-label={name} title={name} data-workbench-target="browser" className="max-w-full justify-start"
          onClick={() => { useBrowserRequests.getState().showBrowser(browser.id); revealView('browser'); }}>
          <span aria-hidden><StatusDot tone={browser.state === 'open' ? 'success' : 'info'} label={state} /></span><span className="min-w-0 truncate">{name}</span>
        </Button></li>;
      })}
    </ul>
    <Button size="sm" variant="ghost" onClick={() => revealView('browser')}>{t('workbench.targets.manageBrowsers')}</Button>
  </div>;
}

export function DisplaysSection() {
  const { t } = useTranslation();
  const shown = useViewShown();
  const state = useThreadDoor(useRouteThread());
  const id = state.kind === 'ready' && state.access.thread.environment_permissions.includes('vnc.view') ? state.access.environment.id : null;
  const query = useQuery({ ...displaysQuery(id ?? ''), enabled: shown && id !== null });
  useTargetsFeed(shown ? id : null, 'displays');
  if (id === null) return null;
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const displays = query.data.displays.filter((display) => display.state !== 'closed');
  return <div className="relative h-full min-h-0 overflow-y-auto p-2">
    {displays.length === 0 && <EmptyState compact title={t('vnc.list.empty')} icon={<MonitorPlay aria-hidden />} />}
    <ul aria-label={t('vnc.list.label')}>
      {displays.map((display) => {
        const state = t(`vnc.state.${display.state}`);
        const name = t('workbench.targetNames.display', { name: display.name, state });
        return <li key={display.id}><Button variant="ghost" size="sm" aria-label={name} title={name} data-workbench-target="display" className="max-w-full justify-start"
          onClick={() => { useBrowserRequests.getState().showDisplay(display.id); revealView('vnc'); }}>
          <span aria-hidden><StatusDot tone={display.state === 'open' ? 'success' : 'info'} label={state} /></span><span className="min-w-0 truncate">{name}</span>
        </Button></li>;
      })}
    </ul>
    <Button size="sm" variant="ghost" onClick={() => revealView('vnc')}>{t('workbench.targets.manageDisplays')}</Button>
  </div>;
}
