/** The workbench as the shell: rail, left bar, main with its groups of tabs, right bar, bottom and status bar.
 *
 * What stands round every screen of somebody who is signed in. With room it is the workbench: the
 * rail, the areas with their borders, the title of the main area and the line at the foot. On a
 * small screen it is one view at a time. The screen of the route is handed to whichever of the two
 * is in use, and shown in the view its reader put it in.
 */
import { useRouterState } from '@tanstack/react-router';
import { PanelBottom, PanelLeft, PanelRight } from 'lucide-react';
import { Activity, Suspense, memo, type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { unclaimedFileDrop } from '@/lib/drop';
import '@/features/chat/register';
import '@/features/documents/register';
import { ExportStatus } from '@/features/export/ExportStatus';
import { ImportStatus } from '@/features/people/ImportStatus';
import { closeFinder } from '@/features/finder/store';
import { registerHelpView } from '@/features/help/register';
import { registerAskView } from '@/features/ask/register';
import { HelpWork } from '@/features/help/HelpWork';
import { useHelpState } from '@/features/help/state';
import { registerInspectorViews } from '@/features/inspector/Inspector';
import type { ViewMenuProps, ViewProps } from '@/features/workbench/views';
import { ArrangementMenu } from '@/features/workbench/Arrangements';
import { useWorkbenchChoiceHelp } from '@/features/workbench/choiceHelp';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { Board, ViewHost } from '@/features/workbench/Board';
import { OpenViewCommands } from '@/features/workbench/OpenViews';
import { useShape } from '@/features/workbench/Narrow';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { ConnectionPart, ThreadStatusParts } from '@/features/workbench/StatusParts';
import { useWorkbench } from '@/features/workbench/store';
import { type BarId, barHasViews } from '@/features/workbench/layout';
import { Banners } from '@/shell/Banners';
import { MobileNavigation } from '@/shell/MobileNavigation';
import { clearSelectedOrg } from '@/shell/org';
import { Rail } from '@/shell/Rail';
import { type KeyActionId, keysAct } from '@/shell/keyActions';
import { useKeyChoices } from '@/shell/keys';
import { IconButton } from '@/ui/IconButton';
import { ScreenTitleBar } from '@/shell/TitleBar';
import { useVisibleLayout, useWorkbenchBars } from '@/features/workbench/availability';
import { StatusBar } from '@/ui/StatusBar';
import { SkipMain } from '@/shell/SkipMain';
import { mark, whenContentShown } from '@/lib/marks';
import { pageHtmlReads } from '@/api/htmlReads';
import { useShellBootstrap } from '@/shell/bootstrap';
import { useKeepShell } from '@/shell/keepShell';
import { uncover } from '@/shell/keptShell';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { ErrorState } from '@/ui/ErrorState';
import { OverlayChoices } from '@/features/workbench/OverlayChoices';
import { PageRouting } from '@/features/workbench/PageRouting';
import { holdViewReveals } from '@/features/workbench/openView';
import { FrameReadiness, useFrameReadiness } from '@/shell/FrameReadiness';

import { lazyPart } from '@/features/files/lazy';
import { ContentBoundary } from '@/ui/ContentBoundary';
import type { FrameServicesProps } from '@/shell/FrameServices';

const FrameServices = lazyPart<FrameServicesProps>(() => import('@/shell/FrameServices'));

const Sidebar = lazyPart<ViewProps>(() => import('@/features/navigation/Sidebar').then((module) => ({ default: module.Sidebar })));
const SidebarActions = lazyPart<Partial<ViewMenuProps>>(() => import('@/features/navigation/Sidebar').then((module) => ({ default: module.NavigationActions })));

function NavigationActions(props: Partial<ViewMenuProps>) {
  return <ContentBoundary><Suspense fallback={null}><SidebarActions {...props} /></Suspense></ContentBoundary>;
}

registerShippedViews(Sidebar, NavigationActions, NavigationActions);
// Each feature supplies its own views; only registered views are offered.
registerHelpView();
registerAskView();
registerInspectorViews();

client.session.subscribe((next, previous) => {
  if (next.userId !== previous.userId) clearViewOpenings();
});

// Whoever was signed in has left: nothing of their workbench stays in the page.
client.onSession((event) => {
  if (event.type !== 'signed_out') return;
  closeFinder();
  useHelpState.setState({ overlay: false, readingRequest: 0, origin: null, settingContext: null, guideContext: null, article: null, query: '' });
  useWorkbench.getState().leave();
  clearSelectedOrg();
});

const BAR_ICONS = { left: PanelLeft, right: PanelRight, bottom: PanelBottom };
/** The bars in the order their switches stand in the title: as they lie on the screen, from left to right. */
const BARS_IN_TITLE = ['left', 'bottom', 'right'] as const;
/** The keyboard action that shows or hides each bar, whose keys its switch shows. */
const BAR_ACTIONS: Record<BarId, KeyActionId> = { left: 'left-bar', right: 'right-bar', bottom: 'bottom' };

/** The title of the main area: where the screen lies, the arrangement, and the three bars. */
function TitleBar({ beside }: { beside: boolean }) {
  const { t } = useTranslation();
  const helpFor = useWorkbenchChoiceHelp();
  const screen = useWorkbench((state) => state.screen);
  const arrangement = useWorkbench((state) => state.arrangement);
  const layout = useWorkbenchBars();
  const overlay = useWorkbench((state) => state.overlay);
  const toggle = useWorkbench((state) => state.toggle);
  const choose = useWorkbench((state) => state.choose);
  const reset = useWorkbench((state) => state.reset);
  const keyChoices = useKeyChoices();
  const bars = BARS_IN_TITLE.filter((bar) => beside || bar !== 'right');

  return (
    <ScreenTitleBar>
      <ArrangementMenu screen={screen} current={arrangement} onChoose={choose} onReset={reset} helpFor={helpFor} />
      {bars.map((bar) => {
        const Icon = BAR_ICONS[bar];
        const holds = barHasViews(layout, bar);
        const shown = bar === 'right' && !beside ? overlay === 'right' : layout[bar].open;
        return (
          <IconButton
            key={bar}
            size="sm"
            label={t(`workbench.bars.toggle.${bar}`)}
            shortcut={keysAct(BAR_ACTIONS[bar], keyChoices) ?? undefined}
            icon={<Icon aria-hidden />}
            pressed={shown}
            aria-disabled={holds ? undefined : true}
            className={holds ? undefined : 'text-text-disabled hover:bg-transparent hover:text-text-disabled'}
            onClick={() => toggle(bar, beside)}
          />
        );
      })}
    </ScreenTitleBar>
  );
}

export function Shell({ leavingAdmin = false }: { children: ReactNode; leavingAdmin?: boolean }) {
  useEffect(() => {
    window.addEventListener('dragover', unclaimedFileDrop);
    window.addEventListener('drop', unclaimedFileDrop);
    return () => { window.removeEventListener('dragover', unclaimedFileDrop); window.removeEventListener('drop', unclaimedFileDrop); };
  }, []);
  const userId = useStore(client.session, (state) => state.userId);
  const bootstrap = useShellBootstrap(userId);
  const ready = bootstrap.data?.person.id === userId;
  const { isError, error, refetch } = bootstrap;
  const frame = useMemo(() => ({ ready, pending: isError
    ? <div className="p-4 sm:p-6"><ErrorState error={error} onRetry={() => void refetch()} /></div> : <ScreenLoading /> }), [ready, isError, error, refetch]);
  if (userId === null) return <><SkipMain /><main id="main-content" data-main-area tabIndex={-1} className="h-dvh bg-bg text-text outline-none"><ScreenLoading /></main></>;
  return <FrameReadiness.Provider value={frame}><ShellFrame key={userId} userId={userId} ready={ready} leavingAdmin={leavingAdmin} /></FrameReadiness.Provider>;
}

/** A bootstrap transition pauses admitted services while preserving their dialogs and drafts. */
function AdmittedServices({ ready, children }: { ready: boolean; children: ReactNode }) {
  const [admitted, setAdmitted] = useState(ready);
  if (ready && !admitted) setAdmitted(true);
  if (!admitted) return null;
  return <Activity mode={ready ? 'visible' : 'hidden'}>{children}</Activity>;
}

function HostedViews() {
  const layout = useVisibleLayout();
  return <ViewHost layout={layout} />;
}

function CloseHelpOnNavigation() {
  const href = useRouterState({ select: (state) => state.location.href });
  const previousHref = useRef(href);
  useLayoutEffect(() => {
    if (previousHref.current !== href) {
      pageHtmlReads()?.contentShown?.();
      if (useHelpState.getState().overlay) useHelpState.setState({ overlay: false });
    }
    previousHref.current = href;
  }, [href]);
  return null;
}

const ShellFrame = memo(function ShellFrame({ userId, leavingAdmin, ready }: { userId: string; leavingAdmin: boolean; ready: boolean }) {
  const frame = useFrameReadiness();
  const [addressed, setAddressed] = useState(false);
  const admitAddress = useCallback(() => setAddressed(true), []);
  const admitted = ready && addressed;
  const [wasReady, setWasReady] = useState(ready);
  if (ready && !wasReady) setWasReady(true);
  const pageAdmitted = addressed && (ready || !wasReady && !leavingAdmin);
  useEffect(() => { mark('start/shell-drawn'); }, []);
  useEffect(() => { if (admitted) { mark('start/shell-ready'); uncover(); } }, [admitted]);
  useEffect(() => { if (pageAdmitted) uncover(); }, [pageAdmitted]);
  useKeepShell(userId, admitted);
  const readerVersion = useStore(client.session, (state) => state.readerVersion ?? 0);
  useEffect(() => {
    if (!import.meta.env.PROD || !('serviceWorker' in navigator)) return;
    const away = new AbortController();
    // The conversation can paint while its optional summary is still loading.
    const startup = pageHtmlReads();
    const address = window.location.href;
    const pageShown = () => whenContentShown(userId, readerVersion, away.signal);
    const content = startup?.afterThreadContent?.then(() => {
      if (startup === pageHtmlReads() && address === window.location.href) return true;
      return pageShown();
    }) ?? pageShown();
    void content.then((shown) => {
      if (shown && !away.signal.aborted) return navigator.serviceWorker.register('/sw.js');
    }).catch(() => undefined);
    return () => away.abort();
  }, [userId, readerVersion]);
  const empty = useWorkbench((state) => state.activeContent === null);
  useLayoutEffect(() => {
    if (admitted && empty) mark('page/content', { user: userId, reader: String(readerVersion), view: '', address: '/workbench', outcome: 'ready' });
  }, [admitted, empty, userId, readerVersion]);
  const viewAdmission = useRef<ReturnType<typeof holdViewReveals> | null>(null);
  useLayoutEffect(() => {
    const admission = holdViewReveals(userId, readerVersion);
    viewAdmission.current = admission;
    return () => { viewAdmission.current = null; admission.release(); };
  }, [userId, readerVersion]);
  useLayoutEffect(() => { if (!admitted) viewAdmission.current?.pause(); }, [admitted, userId, readerVersion]);
  useEffect(() => { if (admitted) viewAdmission.current?.admit(); }, [admitted, userId, readerVersion]);
  const shape = useShape();
  const beside = shape === 'wide';
  const [shortcutList, setShortcutList] = useState(false);
  const showShortcuts = useCallback(() => setShortcutList(true), []);
  const focusMode = useWorkbench((state) => state.focusMode);
  const readiness = useMemo(() => ({ ready: admitted, pageReady: pageAdmitted, pending: frame.pending }), [admitted, pageAdmitted, frame.pending]);

  return (
    <>
      <PageRouting onAddressed={admitAddress} />
      <CloseHelpOnNavigation />
      <FrameReadiness.Provider value={readiness}>
      <AdmittedServices ready={admitted}>
        <ContentBoundary>
          <Suspense fallback={null}><FrameServices shortcutList={shortcutList} onShortcutList={setShortcutList} /></Suspense>
        </ContentBoundary>
      </AdmittedServices>
      <OpenViewCommands />
      <HelpWork>
      <SkipMain />
      <AdmittedServices ready={pageAdmitted}><HostedViews /></AdmittedServices>
      {shape === 'small' ? (
        <MobileNavigation onShortcuts={showShortcuts} pending={leavingAdmin} focusMode={focusMode} />
      ) : (
        <Board
          pending={leavingAdmin ? <ScreenLoading /> : undefined}
          rail={focusMode ? null : <Rail onShortcuts={showShortcuts} />}
          top={focusMode ? null :
            <div data-workbench-chrome className="shrink-0">
              <Banners />
              <TitleBar beside={beside} />
              {!beside && <OverlayChoices />}
            </div>
          }
          status={focusMode ? null :
            <StatusBar>
              <ConnectionPart />
              {admitted && <><ThreadStatusParts /><ExportStatus /><ImportStatus /></>}
            </StatusBar>
          }
        />
      )}
      </HelpWork>
      </FrameReadiness.Provider>
    </>
  );
});
