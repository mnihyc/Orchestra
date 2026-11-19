/** Where the Browser view stands: beside a thread (registered only for a reader who may watch the browsers of its environment, and only where they are on), as the Browsers tab of an environment's page, the "Open" of the agent's tool in a conversation, and a port's "Here in its browser".
 *
 * What the view needs is asked of the thread's environment, never of the thread's own page. A port's
 * address is opened through the same chooser of profiles as any browser, by somebody who may drive
 * there; it makes no opening of the port. The row of the agent's tool brings the view to the front on
 * the browser the answer names.
 */
import { useQuery } from '@tanstack/react-query';
import { AppWindow } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { environmentQuery } from '@/api/environments';
import { useRouteThread, useThreadDoor } from '@/features/files/door';
import type { EnvironmentPort, PortsBrowser } from '@/features/ports/presentation';
import { revealView } from '@/features/workbench/openView';
import { registerView } from '@/features/workbench/views';
import { BrowsersSection } from '@/features/workbench/TargetSections';
import type { Environment } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

import { BrowsersPanel } from './Browsers';
import { useBrowserRequests } from './store';

/** The view's body beside a thread: the browsers of the thread's environment. */
export function BrowserView() {
  const { t } = useTranslation();
  const threadId = useRouteThread();
  const state = useThreadDoor(threadId);
  const id = state.kind === 'ready' ? state.access.environment.id : null;
  const allowed = state.kind === 'ready' && state.access.thread.environment_permissions.includes('browser.view');
  const query = useQuery({ ...environmentQuery(id ?? ''), enabled: id !== null && allowed });
  if (state.kind === 'loading') return <SkeletonRows rows={4} />;
  if (!allowed) return <EmptyState compact className="h-full" icon={<AppWindow aria-hidden />} title={t('browser.view.notAllowed')} />;
  if (query.isPending) return <SkeletonRows rows={4} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  return <BrowsersPanel environment={query.data} threadId={threadId} />;
}

/** Registers the view beside a thread whose environment lets the reader watch its browsers, where they are on. */
export function useBrowserRegistration(): void {
  useEffect(() => {
    const viewer = registerView({ name: 'browser', title: 'browser.view.title', about: 'browser.view.about', icon: AppWindow, screens: ['thread'], area: 'main', body: BrowserView });
    const section = registerView({ name: 'browsers', title: 'workbench.targets.browsers', about: 'browser.view.about', icon: AppWindow, screens: ['thread'], area: 'right', container: 'environment', order: 4, body: BrowsersSection });
    return () => { section(); viewer(); };
  }, []);
}

/** Why "Here in its browser" cannot be used for this environment by this reader, or null when it can. */
export function hereUnavailable(environment: Environment): string | null {
  if (!environment.access.permissions.includes('browser.manage')) return i18n.t('browser.here.manage', { environment: environment.name });
  if (!environment.browser) return i18n.t('browser.here.off', { environment: environment.name });
  if (environment.support.browser.programs.length === 0) return i18n.t('browser.here.none', { environment: environment.name });
  return null;
}

/** The Ports view's "Here in its browser": the local HTTP address of the port, opened with the chooser of profiles. `show` brings the Browser view to the front. */
export function portsBrowser(environment: Environment, show: () => void): PortsBrowser {
  return {
    reason: hereUnavailable(environment),
    open: (port: EnvironmentPort) => {
      useBrowserRequests.getState().openAddress(environment.id, `http://127.0.0.1:${port.port}/`);
      show();
    },
  };
}

/** The Ports view beside a thread brings the Browser view forward there. */
export function showBrowserBeside(): void {
  revealView('browser');
}

/** "Open" on the row of the agent's tool: the browser or display the answer names, in front of its view. */
export function OpenFromTool({ tool, result }: { tool: string; result: unknown }) {
  const { t } = useTranslation();
  if (typeof result !== 'object' || result === null) return null;
  const record = result as Record<string, unknown>;
  const browser = tool === 'use_browser' && typeof record.browser === 'string' ? record.browser : null;
  const display = tool === 'use_display' && typeof record.display === 'string' ? record.display : null;
  if (browser === null && display === null) return null;
  return (
    <Button
      action="open"
      size="sm"
      onClick={() => {
        if (browser !== null) {
          useBrowserRequests.getState().showBrowser(browser);
          revealView('browser');
        } else if (display !== null) {
          useBrowserRequests.getState().showDisplay(display);
          revealView('vnc');
        }
      }}
    >
      {browser !== null ? t('browser.tool.open') : t('vnc.tool.open')}
    </Button>
  );
}
