import { LiveThreadStateIcon } from '@/ui/ThreadState';
import { ArrangementMenu } from '@/features/workbench/Arrangements';
import { useWorkbench } from '@/features/workbench/store';
import { tabEntries } from '@/features/workbench/Tabs';
import { viewOf } from '@/features/workbench/views';
import { ViewTab } from '@/ui/ViewTab';
import { useFrameReady } from '@/shell/FrameReadiness';
/** Below the medium width: bottom tabs, the left bar as a drawer and every other view as a sheet, one at a time.
 *
 * A small screen has no areas to arrange. It shows the screen of the route, a bar of five places at
 * its foot, the navigation as a drawer from the left, and one row of named container icons above
 * whichever sheet its reader opens. Focus and a touch hold reveal each icon's name.
 */
import { Link, useRouterState } from '@tanstack/react-router';
import { House, Inbox, List, MessagesSquare, Search, ShieldCheck, UserRound } from 'lucide-react';
import { type ReactNode } from 'react';
import { useTabCaptions } from '@/features/workbench/TabNames';
import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';

import { orgsQuery } from '@/api/nodes';
import { meQuery } from '@/api/auth';
import { openFinder } from '@/features/finder/store';
import { CountBadge, useCounterWords, useInboxCounters } from '@/features/inbox/Counters';
import { useNarrowWorkbench } from '@/features/workbench/NarrowViews';
import { NarrowChoices } from '@/features/workbench/NarrowChoices';
import { AddViewButton } from '@/features/workbench/OpenViews';
import { Banners } from '@/shell/Banners';
import { ScreenTitleBar } from '@/shell/TitleBar';
import { railOrgs, useSelectedOrg } from '@/shell/org';
import { useAccountMenu } from '@/shell/Rail';
import { IconButton } from '@/ui/IconButton';
import { Menu } from '@/ui/Menu';
import { ScreenLoading } from '@/shell/ScreenLoading';

const TAB =
  'focus-inset flex h-full min-w-0 flex-1 flex-col items-center justify-center gap-0.5 text-xs text-text-muted transition-colors hover:text-text aria-[current=page]:text-accent-text';

/** Icons of navigation are drawn a little larger than those in the dense parts of the interface. */
const NAVIGATION_ICON = 18;

function BottomTab({ icon, badge, children }: { icon: ReactNode; badge?: ReactNode; children: ReactNode }) {
  return (
    <>
      <span className="relative flex">
        {icon}
        {badge}
      </span>
      <span className="max-w-full truncate">{children}</span>
    </>
  );
}


export function MobileNavigation({ onShortcuts, pending = false, focusMode = false }: { onShortcuts: () => void; pending?: boolean; focusMode?: boolean }) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const workbench = useNarrowWorkbench(pending, focusMode);
  const frame = useWorkbench();
  const primary = workbench.primary;
  const captions = useTabCaptions(workbench.mainTabs);
  const named = primary === null ? undefined : frame.named.get(primary.id);
  const definition = primary === null ? undefined : viewOf(primary.view);
  const Icon = named?.icon ?? definition?.icon;
  const title = (primary === null ? undefined : captions.get(primary.id)) ?? named?.title ?? t(definition?.title ?? 'views.screen.title');
  const allOrgs = useQuery({ ...orgsQuery, enabled: ready }).data ?? [];
  const me = useQuery({ ...meQuery, enabled: ready }).data;
  const orgs = railOrgs(allOrgs, me);
  const selected = useSelectedOrg((state) => state.orgId);
  const org = allOrgs.find((candidate) => candidate.id === selected) ?? orgs[0];
  const account = useAccountMenu(onShortcuts);
  const { unread, approvalsPending } = useInboxCounters();
  const words = useCounterWords();
  const inThread = useRouterState({ select: (state) => state.location.pathname.startsWith('/thread/') });

  return (
    <div className="flex h-dvh min-h-0 flex-col bg-bg text-text">
      {!focusMode && <div data-workbench-chrome className="shrink-0">
      <Banners />
      <ScreenTitleBar>
        <IconButton size="sm" label={t('finder.open')} icon={<Search aria-hidden />} onClick={() => openFinder()} />
      </ScreenTitleBar>
      <NarrowChoices choices={workbench.choices} presentation="icons" />
      </div>}
      <div className="flex shrink-0 items-stretch border-b border-border bg-bg-subtle" data-mobile-main-tabs>
        <div role="tablist" aria-label={t('workbench.areas.main')} className="flex min-w-0 flex-1">
          {primary !== null && <ViewTab id={`mobile-tab-${primary.id}`} panelId="mobile-main-panel" title={title} about={named?.path} active tabbable closable={primary.pinned !== true} pinned={primary.pinned === true}
            icon={named?.thread === undefined ? Icon === undefined ? undefined : <Icon aria-hidden /> : <LiveThreadStateIcon display={named.thread.display} threadId={named.thread.id} />} unsaved={frame.unsaved.has(primary.id)}
            menu={tabEntries(frame.layout, primary.id, t, frame.change)} onSelect={() => workbench.selectMain(primary.id)}
            onClose={() => frame.close({ kind: 'tab', tab: primary.id })} />}
        </div>
        <div className="flex h-(--tab-strip-height) shrink-0 items-center">
        {focusMode && <AddViewButton />}
        {workbench.mainTabs.length > 1 && <IconButton size="sm" label={t('workbench.allTabs', { name: t('workbench.areas.main') })} icon={<List aria-hidden />} onClick={() => openFinder('', 'workspace.focus.')} />}
        <ArrangementMenu screen={frame.screen} current={frame.arrangement} onChoose={frame.choose} onReset={frame.reset} />
        </div>
      </div>
      <main id="main-content" data-mobile-main data-main-area data-area-stop tabIndex={-1} inert={!pending && workbench.main === null} aria-hidden={!pending && workbench.main === null ? true : undefined} aria-label={t('workbench.areas.main')} className="relative min-h-0 flex-1 overflow-hidden outline-none">{pending ? <ScreenLoading /> : <div id="mobile-main-panel" role={primary === null ? undefined : 'tabpanel'} aria-labelledby={primary === null ? undefined : `mobile-tab-${primary.id}`} className="h-full min-h-0">{workbench.main}</div>}</main>
      {!focusMode && !inThread && <nav data-navigation-rail data-area-stop inert={workbench.main === null} aria-hidden={workbench.main === null ? true : undefined}
        aria-label={t('shell.rail.label')}
        className="flex h-(--bottom-tabs-height) shrink-0 items-stretch border-t border-border bg-bg-subtle"
      ><ul className="flex h-full w-full min-w-0 items-stretch">
        <li className="flex min-w-0 flex-1">{org === undefined ? <Link to="/" className={TAB}>
          <BottomTab icon={<House size={NAVIGATION_ICON} aria-hidden />}>{t('shell.small.home')}</BottomTab>
        </Link> : <Link to="/org/$slug" params={{ slug: org.address }} title={t('workbench.orgActions.homeName', { name: org.name })} className={TAB} activeOptions={{ exact: true }}>
          <BottomTab icon={<House size={NAVIGATION_ICON} aria-hidden />}>{t('shell.small.home')}</BottomTab>
        </Link>}</li>
        <li className="flex min-w-0 flex-1"><Link to="/inbox" className={TAB} aria-label={words.unread === null ? t('screens.inbox.title') : `${t('screens.inbox.title')}, ${words.unread}`}>
          <BottomTab
            icon={<Inbox size={NAVIGATION_ICON} aria-hidden />}
            badge={<CountBadge count={unread} tone="accent" label={null} className="absolute -top-2 -right-3" />}
          >
            {t('screens.inbox.title')}
          </BottomTab>
        </Link></li>
        {org !== undefined && (
          <li className="flex min-w-0 flex-1"><Link to="/org/$slug/messages" params={{ slug: org.slug }} className={TAB}>
            <BottomTab icon={<MessagesSquare size={NAVIGATION_ICON} aria-hidden />}>{t('screens.messages.title')}</BottomTab>
          </Link></li>
        )}
        <li className="flex min-w-0 flex-1"><Link to="/approvals" className={TAB} aria-label={words.approvals === null ? t('screens.approvals.title') : `${t('screens.approvals.title')}, ${words.approvals}`}>
          <BottomTab
            icon={<ShieldCheck size={NAVIGATION_ICON} aria-hidden />}
            badge={<CountBadge count={approvalsPending} tone="warning" label={null} className="absolute -top-2 -right-3" />}
          >
            {t('screens.approvals.title')}
          </BottomTab>
        </Link></li>
        <li className="flex min-w-0 flex-1"><Menu
          label={t('shell.account.label')}
          side="top"
          align="end"
          entries={account}
          trigger={
            <button type="button" className={TAB}>
              <BottomTab icon={<UserRound size={NAVIGATION_ICON} aria-hidden />}>{t('shell.small.me')}</BottomTab>
            </button>
          }
        /></li>
      </ul></nav>}

      {workbench.sheets}
    </div>
  );
}

