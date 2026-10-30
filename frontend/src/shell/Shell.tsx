/** The workbench as the shell: rail, left bar, main with its groups of tabs, right bar, bottom and status bar.
 *
 * What stands round every screen of somebody who is signed in. With room it is the workbench: the
 * rail, the areas with their borders, the title of the main area and the line at the foot. On a
 * small screen it is one view at a time. The screen of the route is handed to whichever of the two
 * is in use, and shown in the view its reader put it in.
 */
import { PanelTargetChoice } from '@/features/workbench/panelScope';
import { CloseViews } from '@/features/workbench/CloseViews';
import { WorkspaceCommands } from '@/features/workbench/workspaceActions';
import { useRouterState } from '@tanstack/react-router';
import { PanelBottom, PanelLeft, PanelRight } from 'lucide-react';
import { Activity, type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { unclaimedFileDrop } from '@/lib/drop';
import '@/features/chat/register';
import { useChatLinks } from '@/features/chat/links';
import '@/features/documents/register';
import { ExportHost } from '@/features/export/ExportHost';
import { ExportStatus } from '@/features/export/ExportStatus';
import { ImportHost } from '@/features/people/ImportHost';
import { ImportStatus } from '@/features/people/ImportStatus';
import { closeFinder } from '@/features/finder/store';
import { ShellCommands } from '@/features/finder/shellCommands';
import { registerHelpView } from '@/features/help/register';
import { TutorialHost } from '@/features/help/Tutorial';
import { HelpOverlayHost, HelpWork } from '@/features/help/HelpOverlay';
import { useHelpState } from '@/features/help/state';
import { AccessHost } from '@/features/access/AccessDialog';
import { registerInspectorViews } from '@/features/inspector/Inspector';
import { usePortsRegistration } from '@/features/ports/View';
import { useBrowserRegistration } from '@/features/browser/View';
import { useVncRegistration } from '@/features/vnc/View';
import { ChangeHost } from '@/features/change/ChangeHost';
import { useRememberPlace } from '@/features/navigation/lastPlace';
import { NodeDialogHost } from '@/features/navigation/NodeDialogs';
import { CapHost } from '@/features/usage/AddCap';
import { NavigationActions, Sidebar } from '@/features/navigation/Sidebar';
import { useVisitRecorder } from '@/features/navigation/visits';
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
import { useChatActivity } from '@/realtime/stores/chat';
import { Banners } from '@/shell/Banners';
import { MobileNavigation } from '@/shell/MobileNavigation';
import { clearSelectedOrg } from '@/shell/org';
import { Rail } from '@/shell/Rail';
import { type KeyActionId, keysAct } from '@/shell/keyActions';
import { useKeyChoices } from '@/shell/keys';
import { Shortcuts } from '@/shell/Shortcuts';
import { IconButton } from '@/ui/IconButton';
import { ScreenTitleBar } from '@/shell/TitleBar';
import { useVisibleLayout } from '@/features/workbench/availability';
import { StatusBar } from '@/ui/StatusBar';
import { SkipMain } from '@/shell/SkipMain';
import { useShellBootstrap } from '@/shell/bootstrap';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { ErrorState } from '@/ui/ErrorState';
import { OverlayChoices } from '@/features/workbench/OverlayChoices';
import { PageRouting } from '@/features/workbench/PageRouting';
import { FrameReadiness, useFrameReadiness } from '@/shell/FrameReadiness';

registerShippedViews(Sidebar, NavigationActions, NavigationActions);
// Each feature supplies its own views; only registered views are offered.
registerHelpView();
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
  const layout = useVisibleLayout();
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

export function Shell({ children, leavingAdmin = false }: { children: ReactNode; leavingAdmin?: boolean }) {
  useEffect(() => {
    window.addEventListener('dragover', unclaimedFileDrop);
    window.addEventListener('drop', unclaimedFileDrop);
    return () => { window.removeEventListener('dragover', unclaimedFileDrop); window.removeEventListener('drop', unclaimedFileDrop); };
  }, []);
  const userId = useStore(client.session, (state) => state.userId);
  const bootstrap = useShellBootstrap(userId);
  if (userId === null) return <><SkipMain /><main id="main-content" data-main-area tabIndex={-1} className="h-dvh bg-bg text-text outline-none"><ScreenLoading /></main></>;
  const ready = bootstrap.data?.person.id === userId;
  const pending = bootstrap.isError ? <div className="p-4 sm:p-6"><ErrorState error={bootstrap.error} onRetry={() => void bootstrap.refetch()} /></div> : <ScreenLoading />;
  return <FrameReadiness.Provider value={{ ready, pending }}><ShellFrame key={userId} ready={ready} leavingAdmin={leavingAdmin}>{children}</ShellFrame></FrameReadiness.Provider>;
}

/** A bootstrap transition pauses admitted services while preserving their dialogs and drafts. */
function AdmittedServices({ ready, children }: { ready: boolean; children: ReactNode }) {
  const [admitted, setAdmitted] = useState(ready);
  if (ready && !admitted) setAdmitted(true);
  if (!admitted) return null;
  return <Activity mode={ready ? 'visible' : 'hidden'}>{children}</Activity>;
}

function FrameServices() {
  useChatLinks();
  usePortsRegistration();
  useBrowserRegistration();
  useVncRegistration();
  useVisitRecorder();
  useRememberPlace();
  useChatActivity();
  return <><ShellCommands /><WorkspaceCommands /><TutorialHost /></>;
}

function ShellFrame({ leavingAdmin, ready }: { children: ReactNode; leavingAdmin: boolean; ready: boolean }) {
  const frame = useFrameReadiness();
  const [addressed, setAddressed] = useState(false);
  const admitAddress = useCallback(() => setAddressed(true), []);
  const admitted = ready && addressed;
  const href = useRouterState({ select: (state) => state.location.href });
  const previousHref = useRef(href);
  useLayoutEffect(() => {
    if (previousHref.current !== href && useHelpState.getState().overlay) useHelpState.setState({ overlay: false });
    previousHref.current = href;
  }, [href]);
  const shape = useShape();
  const visibleLayout = useVisibleLayout();
  const beside = shape === 'wide';
  const [shortcutList, setShortcutList] = useState(false);
  const showShortcuts = useCallback(() => setShortcutList(true), []);
  const focusMode = useWorkbench((state) => state.focusMode);

  return (
    <>
      <PageRouting ready={ready} onAddressed={admitAddress} />
      <FrameReadiness.Provider value={{ ready: admitted, pending: frame.pending }}>
      <AdmittedServices ready={admitted}><FrameServices />
      <HelpOverlayHost />
      <Shortcuts listOpen={shortcutList} onListOpenChange={setShortcutList} />
      <NodeDialogHost />
      <CapHost />
      <ChangeHost />
      <AccessHost />
      <ExportHost /><ImportHost /><CloseViews /><PanelTargetChoice />
      </AdmittedServices>
      <OpenViewCommands />
      <HelpWork>
      <SkipMain />
      <AdmittedServices ready={admitted}><ViewHost layout={visibleLayout} /></AdmittedServices>
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
}
