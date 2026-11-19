import { useFrameReady } from '@/shell/FrameReadiness';
/** Org avatars, the person's own first, the finder, inbox, approvals, resources, help and the user's menu.
 *
 * The one part of the workbench that never moves: the orgs a person belongs to, their own first,
 * and under them the places that belong to the person and not to an org, with the menu of their
 * own account at the foot.
 *
 * The orgs, each with a dot while one of the person's own chats there is unread; after the last of
 * them what was shared with the person from elsewhere, and the "+" that makes an org or joins one by
 * a link; then the finder, Inbox and Approvals with what waits there, Resources, Help and the
 * account's menu. Every org a person belongs to is one press away, their own first, and two whose
 * names begin alike are told apart by a second letter and by their colours. Inbox and Approvals stand
 * here and nowhere else, and the hint of each names its key.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useDroppable } from '@dnd-kit/core';
import { Link, useNavigate, useRouterState } from '@tanstack/react-router';
import { Boxes, ChartColumn, CircleQuestionMark, HardDrive, Inbox, Keyboard, Link2, ListTree, LogOut, Monitor, Moon, Plus, Search, Settings, Share2, ShieldCheck, ShieldUser, Sun, UsersRound } from 'lucide-react';
import { type KeyboardEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { screenBuilt } from '@/shell/availability';

import { meKey, meQuery, metaQuery, updateMe } from '@/api/auth';
import { useManagementAvailable } from '@/features/access/AdminMode';
import { orgsQuery } from '@/api/nodes';
import { client } from '@/api/client';
import { openFinder } from '@/features/finder/store';
import { CountBadge, useCounterWords, useInboxCounters } from '@/features/inbox/Counters';
import { useUnreadChats } from '@/features/navigation/counts';
import { useNavigationOrg } from '@/features/navigation/currentOrg';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { orgColor, orgMarks } from '@/features/navigation/OrgSwitcher';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { RESOURCES_VIEW } from '@/features/navigation/Resources';
import { findView, type Layout, move, open, selectContainer, type Target } from '@/features/workbench/layout';
import { spotId, type DropSpot } from '@/features/workbench/MoveView';
import { containerOf, FALLBACK_CONTAINER_ICON } from '@/features/workbench/containers';
import { viewOf } from '@/features/workbench/views';
import { useWorkbench } from '@/features/workbench/store';
import { requestViewAction } from '@/features/workbench/CloseViews';
import type { Density, Locale, Me, MeUpdateBody, Org, Theme } from '@/generated/api-types';
import { LOCALES } from '@/i18n';
import { adoptAppearance, useAppearance } from '@/lib/appearance';
import { leave } from '@/routes/-access';
import { useActionKeys } from '@/shell/keys';
import { railOrgs, selectOrg, useSelectedOrg } from '@/shell/org';
import { Avatar } from '@/ui/Avatar';
import { cx } from '@/ui/cx';
import { useErrorText } from '@/ui/errorText';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { Tooltip } from '@/ui/Tooltip';
import { containerChoice, viewChoice } from '@/features/workbench/choiceWords';
import { useCurrentHeading } from '@/shell/screen';
import { useNodePath } from '@/realtime/stores/tree';

/** Icons of navigation are drawn a little larger than those in the dense parts of the interface. */
const NAVIGATION_ICON = 18;

/** The direct shared screen has no offer until its body is built. */
const SHARED_BUILT = screenBuilt('/shared');

const STOP =
  'relative flex size-10 items-center justify-center rounded-lg text-text-muted transition-colors hover:bg-bg-muted hover:text-text';

/** The account's settings, quick appearance choices, shortcuts and sign-out. */
export function useAccountMenu(onShortcuts: () => void): MenuEntry[] {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const describe = useErrorText();
  const theme = useAppearance((state) => state.theme);
  const density = useAppearance((state) => state.density);
  const locale = useAppearance((state) => state.locale);
  const set = useAppearance((state) => state.set);
  const administers = useManagementAvailable();
  const listKeys = useActionKeys('list');

  // A preference is the person's own and easily undone: it shows at once and is put right if the server refuses.
  const save = useMutation({
    mutationFn: (change: MeUpdateBody) => updateMe(change),
    onError: (error) => {
      // What the server holds stands.
      const me = queryClient.getQueryData<Me>(meKey);
      if (me !== undefined) adoptAppearance(me);
      toast({ tone: 'danger', title: t('shell.account.notSaved'), description: describe(error).message });
    },
  });

  const out = useMutation({
    mutationFn: leave,
    onError: (error) => toast({ tone: 'danger', title: t('shell.account.notSignedOut'), description: describe(error).message }),
  });

  const entries: MenuEntry[] = [
    {
      id: 'settings',
      label: t('screens.sections.settings'),
      icon: <Settings aria-hidden />,
      onSelect: () => void navigate({ to: '/settings/profile' }),
    },
    { id: 'usage', label: t('usage.screenTitle'), icon: <ChartColumn aria-hidden />, onSelect: () => void navigate({ to: '/usage', search: {} }) },
    { id: 'storage', label: t('storage.title'), icon: <HardDrive aria-hidden />, onSelect: () => void navigate({ to: '/storage', search: {} }) },
    // The destination stays available to a system admin; opening its data still needs native mode.
    ...(administers
      ? [{ id: 'management', label: t('screens.admin.title'), icon: <ShieldUser aria-hidden />, onSelect: () => void navigate({ to: '/admin/$section', params: { section: 'overview' } }) }]
      : []),
    { id: 'shortcuts', label: t('shell.shortcuts.title'), icon: <Keyboard aria-hidden />, shortcut: listKeys, onSelect: onShortcuts },
    { type: 'separator', id: 'separator-theme' },
    { type: 'label', id: 'theme-label', label: t('preferences.theme') },
    {
      type: 'radio',
      id: 'theme',
      label: t('preferences.theme'),
      value: theme,
      onValueChange: (value) => {
        set({ theme: value as Theme });
        save.mutate({ theme: value as Theme });
      },
      options: [
        { value: 'system', label: t('preferences.themeSystem'), icon: <Monitor aria-hidden /> },
        { value: 'light', label: t('preferences.themeLight'), icon: <Sun aria-hidden /> },
        { value: 'dark', label: t('preferences.themeDark'), icon: <Moon aria-hidden /> },
      ],
    },
    { type: 'separator', id: 'separator-density' },
    { type: 'label', id: 'density-label', label: t('preferences.density') },
    {
      type: 'radio', id: 'density', label: t('preferences.density'), value: density,
      onValueChange: (value) => { set({ density: value as Density }); save.mutate({ density: value as Density }); },
      options: [
        { value: 'comfortable', label: t('preferences.densityComfortable') },
        { value: 'compact', label: t('preferences.densityCompact') },
      ],
    },
    { type: 'separator', id: 'separator-language' },
    { type: 'label', id: 'language-label', label: t('preferences.language') },
    {
      type: 'radio', id: 'language', label: t('preferences.language'), value: locale,
      onValueChange: (value) => { set({ locale: value as Locale }); save.mutate({ locale: value as Locale }); },
      options: LOCALES.map((entry) => ({ value: entry.locale, label: entry.name })),
    },
    { type: 'separator', id: 'separator-out' },
    { id: 'sign-out', label: t('shell.account.signOut'), icon: <LogOut aria-hidden />, onSelect: () => requestViewAction([...useWorkbench.getState().unsaved], () => out.mutate()) },
  ];
  return ready ? entries : entries.filter((entry) => ['settings', 'usage', 'sign-out'].includes(entry.id));
}

/** The two views of the left bar that the rail leads to: the tree of an org, and the resources. */
const NAVIGATION_VIEW = 'navigation';
const RAIL_VIEWS: readonly string[] = [NAVIGATION_VIEW, RESOURCES_VIEW];

/** The view at the front of the left bar. */
function frontOf(layout: Layout): string | null {
  return layout.left.active;
}

/** Brings one of the rail's two views to the front of the left bar, which opens for it. */
function bringToFront(view: string, toggle = false): void {
  useWorkbench.getState().change((layout) => {
    if (layout.left.containers.some((container) => container.name === view)) {
      return selectContainer(layout, 'left', view, toggle);
    }
    const opened = open(layout, view, 'left', undefined, view);
    const place = findView(opened, view);
    return place === null || place.area === 'left' ? opened : move(opened, place.tab.id, { to: 'container', bar: 'left', container: view });
  });
  useWorkbench.getState().setOverlay(null);
}

/**
 * Keeps the view the reader chose in the rail at the front of the left bar from one kind of screen to the
 * next. The workbench holds an arrangement for each kind of screen, so the bar of the screen arrived at
 * would show what it showed last time: what a resource's row leads to would put the list of resources away.
 * A bar that is closed on the screen arrived at stays closed.
 */
function useLeftBarFollows(): void {
  useEffect(() => {
    let screen = useWorkbench.getState().screen;
    let wanted = frontOf(useWorkbench.getState().layout);
    return useWorkbench.subscribe((state) => {
      const front = frontOf(state.layout);
      if (state.screen === screen) {
        // On the same screen whatever is in front is what its reader put there.
        wanted = front;
        return;
      }
      screen = state.screen;
      if (wanted === null || wanted === front || !state.layout.left.open || !state.layout.left.containers.some((container) => container.name === wanted)) return;
      // The navigation is brought to the front only where the bar has it: a reader who took it out is not given it back.
      if (wanted === NAVIGATION_VIEW && findView(state.layout, NAVIGATION_VIEW) === null) return;
      bringToFront(wanted);
    });
  }, []);
}

function RailLink({ label, shortcut, children, ...link }: { label: string; shortcut?: string; children: ReactNode; to: '/inbox' | '/approvals' | '/shared' | '/help' | '/everything' }) {
  return (
    <Tooltip content={label} shortcut={shortcut} side="right">
      <Link {...link} aria-label={label} className={STOP} activeOptions={{ exact: false }}>
        {({ isActive }) => <>{isActive && <span aria-hidden data-rail-page className="absolute inset-y-2 -left-2 w-0.5 rounded-r-full bg-accent" />}{children}</>}
      </Link>
    </Tooltip>
  );
}

/** Inbox or Approvals: the place with its number, which is named with what it counts ("Inbox, 3 unread"), and the key that goes there in its hint. */
function CountedLink({
  to,
  count,
  tone,
  words,
  label,
  shortcut,
  children,
}: {
  to: '/inbox' | '/approvals';
  count: number;
  tone: 'accent' | 'warning';
  /** What the number counts, for the name of the link; `null` for zero. */
  words: string | null;
  label: string;
  /** The keys that go there now; none shown where the person took them away. */
  shortcut: string | undefined;
  children: ReactNode;
}) {
  return (
    <RailLink to={to} shortcut={shortcut} label={words === null ? label : `${label}, ${words}`}>
      {children}
      <CountBadge count={count} tone={tone} label={null} className="absolute top-0.5 right-0.5" />
    </RailLink>
  );
}

/** One org of the rail: its mark, with a line beside it where it is the one in view, and a dot while a chat of the person's own is unread there. */
function OrgStop({ org, mark, current, pageCurrent, homeInView, called }: { org: Org; mark: string; current: boolean; pageCurrent: boolean; homeInView: boolean; called: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const unread = useUnreadChats(org.id) > 0;
  const name = org.org_kind === 'personal' ? t('navigation.ownOrg', { name: called }) : called;
  const label = unread ? t('navigation.orgMark.unread', { name }) : name;
  const description = t(homeInView ? 'workbench.orgActions.toggleDescription' : 'workbench.orgActions.description', { name });
  const target: Target = { to: 'container', bar: 'left', container: NAVIGATION_VIEW };
  const { setNodeRef, isOver } = useDroppable({ id: `org:${org.id}:${spotId(target)}`, data: { target, label: t('views.navigation.title') } satisfies DropSpot });
  return (
    <li ref={setNodeRef} className={cx('relative flex w-full justify-center', isOver && 'ring-2 ring-inset ring-accent')}>
      {pageCurrent && <span aria-hidden data-rail-page className="absolute inset-y-2 left-0 w-0.5 rounded-r-full bg-accent" />}
      <Tooltip content={t(homeInView ? current ? 'workbench.orgActions.hideName' : 'workbench.orgActions.showName' : 'workbench.orgActions.homeName', { name })} side="right">
        <Link
          to="/org/$slug"
          params={{ slug: org.address }}
          aria-label={label}
          aria-description={description}
          aria-current={pageCurrent ? 'page' : false}
          aria-expanded={current}
          activeProps={{ 'aria-current': pageCurrent ? 'page' : false }}
          data-active-left-view={current ? NAVIGATION_VIEW : undefined}
          onClick={(event) => {
            if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
            event.preventDefault();
            const userId = client.session.getState().userId;
            if (homeInView) {
              selectOrg(userId, org.id, true);
              bringToFront(NAVIGATION_VIEW, current);
              return;
            }
            // Mark the chosen destination immediately, including while its screen code loads.
            selectOrg(userId, org.id, true);
            bringToFront(NAVIGATION_VIEW);
            const screen = findView(useWorkbench.getState().layout, 'screen');
            if (screen !== null) useWorkbench.getState().setNamed(screen.tab.id, { title: org.name });
            void navigate({ to: '/org/$slug', params: { slug: org.address } }).then(() => {
              if (userId === null || client.session.getState().userId !== userId) return;
              if (window.location.pathname.replace(/\/$/, '') !== `/org/${encodeURIComponent(org.address)}`) return;
              // Enter before opening: the destination may have a saved arrangement with its bar closed.
              useWorkbench.getState().enter(userId, 'org');
              selectOrg(userId, org.id, true);
              bringToFront(NAVIGATION_VIEW);
            }).catch(() => undefined); // The router renders its navigation failure in the page.
          }}
          className={cx('relative flex size-10 items-center justify-center rounded-lg transition-colors hover:bg-bg-muted', current && 'bg-accent-subtle text-accent-text')}
        >
          <Avatar text={mark} color={orgColor(org)} kind={org.org_kind === 'personal' ? 'human' : 'org'} size={32} />
          {unread && <span aria-hidden data-unread-chats className="absolute top-0.5 right-0.5 size-3 rounded-full border-2 border-bg-subtle bg-accent" />}
        </Link>
      </Tooltip>
    </li>
  );
}

function ActivityDrop({ name, label, children }: { name: string; label: string; children: ReactNode }) {
  const target: Target = { to: 'container', bar: 'left', container: name };
  const { setNodeRef, isOver } = useDroppable({ id: `activity:${spotId(target)}`, data: { target, label } satisfies DropSpot });
  return <div ref={setNodeRef} className={cx('flex justify-center', isOver && 'ring-2 ring-inset ring-accent')}>{children}</div>;
}

/**
 * Why the person may not make a team org now, by the system's `orgs.created_by`; `null` while they may,
 * or while it is not known yet (the dialog asks again). The "+" then reads Join an org: its New org
 * entry stays, greyed with these words, beside joining by a link and the way to the help.
 */
export function useOrgMaking(): string | null {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const meta = useQuery({ ...metaQuery, enabled: ready }).data;
  if (meta === undefined || meta.may_create_org) return null;
  return meta.org_creation === 'admins' ? t('shell.rail.orgCreation.admins') : t('shell.rail.orgCreation.invited');
}

export function Rail({ onShortcuts }: { onShortcuts: () => void }) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const allOrgs = useQuery({ ...orgsQuery, enabled: ready }).data;
  const me = useQuery({ ...meQuery, enabled: ready }).data;
  const orgs = useMemo(() => railOrgs(allOrgs ?? [], me), [allOrgs, me]);
  const joinOnly = useOrgMaking();
  const selected = useSelectedOrg((state) => state.orgId);
  const navigationOrg = useNavigationOrg();
  const heading = useCurrentHeading();
  const pagePath = useNodePath(heading?.nodeId);
  const pageOrg = pagePath.orgId ?? heading?.path?.find((node) => node.kind === 'org')?.id ?? (heading?.nodeId === selected ? selected : null);
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const resourcePage = pathname === '/resources' || pathname.startsWith('/resources/');
  const displayedHome = pathname.replace(/\/$/, '');
  const account = useAccountMenu(onShortcuts);
  const marks = useMemo(() => orgMarks(orgs), [orgs]);
  // Two orgs of one name are told apart by their addresses: "Acme · acme" and "Acme · acme-eu".
  const placed = useMemo(() => orgs.map((org) => ({ id: org.id, name: org.name, above: [], address: org.address })), [orgs]);
  const labels = usePlaceLabels(placed, { trees: false });
  // Keep the server's stable organization order, with this person's space first.
  const ordered = useMemo(
    () => [...orgs].sort((one, other) => Number(other.org_kind === 'personal') - Number(one.org_kind === 'personal')),
    [orgs],
  );
  const resourcesInFront = useWorkbench((state) => state.layout.left.open && frontOf(state.layout) === RESOURCES_VIEW);
  const left = useWorkbench((state) => state.layout.left);
  const navigationInFront = left.open && left.active === NAVIGATION_VIEW;
  const extraContainers = left.containers.filter((container) => !RAIL_VIEWS.includes(container.name));
  const toolbar = useRef<HTMLDivElement>(null);
  const [addOpen, setAddOpen] = useState(false);
  const stops = useCallback(() => Array.from(toolbar.current?.querySelectorAll<HTMLElement>('a[href], button:not([disabled])') ?? []), []);
  const rove = useCallback((chosen: HTMLElement) => { for (const stop of stops()) stop.tabIndex = stop === chosen ? 0 : -1; }, [stops]);
  useLayoutEffect(() => {
    const entries = stops();
    const chosen = entries.find((entry) => entry === document.activeElement) ?? entries.find((entry) => entry.hasAttribute('data-active-left-view')) ?? entries.find((entry) => entry.getAttribute('aria-current') === 'page') ?? entries[0];
    if (chosen !== undefined) rove(chosen);
  }, [orgs, left, stops, rove]);
  const onToolbarKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || event.ctrlKey || event.altKey || event.metaKey) return;
    const entries = stops();
    const index = entries.indexOf(event.target as HTMLElement);
    if (index < 0) return;
    if ((event.target as HTMLElement).hasAttribute('data-org-add') && event.key === 'ArrowRight') {
      event.preventDefault(); event.stopPropagation(); setAddOpen(true); return;
    }
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? entries.length - 1 : ['ArrowDown', 'ArrowRight'].includes(event.key) ? (index + 1) % entries.length : ['ArrowUp', 'ArrowLeft'].includes(event.key) ? (index - 1 + entries.length) % entries.length : null;
    if (next === null) return;
    event.preventDefault();
    const chosen = entries[next];
    event.stopPropagation();
    if (chosen !== undefined) { rove(chosen); chosen.focus(); }
  };
  const { unread, approvalsPending } = useInboxCounters();
  const words = useCounterWords();
  // The keys shown beside the rail's ways are the ones that do them now.
  const finderKeys = useActionKeys('finder');
  const inboxKeys = useActionKeys('inbox');
  const approvalsKeys = useActionKeys('approvals');
  useLeftBarFollows();

  return (
    <nav
      aria-label={t('shell.rail.label')}
      data-navigation-rail
      data-area-stop
      className="flex w-(--rail-width) shrink-0 flex-col items-center gap-1 border-r border-border bg-bg-subtle py-2"
    >
      <div ref={toolbar} role="toolbar" aria-label={t('shell.rail.label')} aria-orientation="vertical" className="contents" onKeyDownCapture={(event) => { if (!(event.target as HTMLElement).closest('[role="menu"]')) onToolbarKey(event); }} onFocusCapture={(event) => {
        const stop = (event.target as HTMLElement).closest<HTMLElement>('a[href],button');
        if (stop !== null && toolbar.current?.contains(stop)) rove(stop);
      }}>
      <ul className="flex min-h-0 w-full flex-1 flex-col items-center gap-1 overflow-y-auto [scrollbar-width:none]">
        {ordered.map((org) => (
          <OrgStop key={org.id} org={org} mark={marks.get(org.id) ?? ''} current={navigationInFront && org.id === navigationOrg} pageCurrent={org.id === pageOrg} homeInView={displayedHome === `/org/${encodeURIComponent(org.address)}`} called={labels.get(org.id) ?? org.name} />
        ))}
        {SHARED_BUILT && <li className="flex w-full justify-center">
          <RailLink to="/shared" label={t('screens.shared.title')}>
            <Share2 size={NAVIGATION_ICON} aria-hidden />
          </RailLink>
        </li>}
        <li className="flex w-full justify-center">
          <Menu
            open={addOpen}
            onOpenChange={setAddOpen}
            label={joinOnly === null ? t('navigation.rail.add') : t('shell.rail.joinOrg')}
            side="right"
            align="start"
            entries={[
              {
                id: 'new-org',
                label: t('shell.rail.newOrg'),
                icon: <UsersRound aria-hidden />,
                disabled: joinOnly ?? false,
                onSelect: () => openNodeDialog({ type: 'new-org' }),
              },
              { id: 'join', label: t('shell.rail.join'), icon: <Link2 aria-hidden />, onSelect: () => openNodeDialog({ type: 'join' }) },
              ...(joinOnly === null
                ? []
                : [
                    {
                      id: 'org-help',
                      label: t('shell.rail.orgCreation.help'),
                      icon: <CircleQuestionMark aria-hidden />,
                      onSelect: () => void navigate({ to: '/help/$article', params: { article: 'getting-started' } }),
                    },
                  ]),
            ]}
            trigger={
              <button type="button" data-org-add aria-label={joinOnly === null ? t('navigation.rail.add') : t('shell.rail.joinOrg')} className={STOP}>
                <Plus size={NAVIGATION_ICON} aria-hidden />
              </button>
            }
          />
        </li>
      </ul>

      <Tooltip content={t('finder.open')} shortcut={finderKeys} side="right">
        <button type="button" aria-label={t('finder.open')} className={STOP} onClick={() => openFinder()}>
          <Search size={NAVIGATION_ICON} aria-hidden />
        </button>
      </Tooltip>
      <RailLink to="/everything" label={t('everything.title')}>
        <ListTree size={NAVIGATION_ICON} aria-hidden />
      </RailLink>
      <CountedLink to="/inbox" count={unread} tone="accent" words={words.unread} label={t('screens.inbox.title')} shortcut={inboxKeys}>
        <Inbox size={NAVIGATION_ICON} aria-hidden />
      </CountedLink>
      <CountedLink to="/approvals" count={approvalsPending} tone="warning" words={words.approvals} label={t('screens.approvals.title')} shortcut={approvalsKeys}>
        <ShieldCheck size={NAVIGATION_ICON} aria-hidden />
      </CountedLink>
      <ActivityDrop name={RESOURCES_VIEW} label={t('workbench.choices.containers.resource-navigation.name')}><Tooltip content={t(resourcesInFront ? 'workbench.bars.hide.resources' : 'workbench.bars.show.resources')} side="right">
        <button type="button" aria-label={t('workbench.choices.containers.resource-navigation.name')} aria-description={t('workbench.choices.containers.resource-navigation.description')} aria-current={resourcePage ? 'page' : undefined} aria-pressed={resourcesInFront} data-active-left-view={resourcesInFront ? RESOURCES_VIEW : undefined} className={cx(STOP, 'aria-pressed:bg-accent-subtle aria-pressed:text-accent-text')} onClick={() => bringToFront(RESOURCES_VIEW, true)}>
          {resourcePage && <span aria-hidden data-rail-page className="absolute inset-y-2 -left-2 w-0.5 rounded-r-full bg-accent" />}
          <Boxes size={NAVIGATION_ICON} aria-hidden />
        </button>
      </Tooltip></ActivityDrop>
      {extraContainers.map((container) => {
        const definition = containerOf(container.name);
        const view = viewOf(container.sections[0]?.tab.view ?? '');
        const choice = definition === undefined && view !== undefined ? viewChoice(view, t) : containerChoice(container.name, t);
        const name = choice.name;
        const Icon = definition?.icon ?? view?.icon ?? FALLBACK_CONTAINER_ICON;
        const active = left.open && left.active === container.name;
        return <ActivityDrop key={container.id} name={container.name} label={name}><Tooltip content={t(active ? 'workbench.bars.hideView' : 'workbench.bars.showView', { name })} side="right"><button type="button" aria-label={name} aria-description={choice.description} aria-pressed={active} data-active-left-view={active ? container.name : undefined} className={cx(STOP, 'aria-pressed:bg-accent-subtle aria-pressed:text-accent-text')} onClick={() => bringToFront(container.name, true)}><Icon size={NAVIGATION_ICON} aria-hidden /></button></Tooltip></ActivityDrop>;
      })}
      <RailLink to="/help" label={t('screens.help.title')}>
        <CircleQuestionMark size={NAVIGATION_ICON} aria-hidden />
      </RailLink>

      <Menu
        label={t('shell.account.label')}
        side="right"
        align="end"
        entries={
          me === undefined
            ? account
            : [{ type: 'label', id: 'who', label: `${me.display_name} · @${me.username}` }, ...account]
        }
        trigger={
          <button type="button" aria-label={t('shell.account.label')} className="mt-1 flex size-10 items-center justify-center rounded-lg transition-colors hover:bg-bg-muted">
            {me !== undefined && (
              <Avatar text={me.avatar.text} color={me.avatar.color} kind={me.kind === 'service' ? 'service' : 'human'} size={32} />
            )}
          </button>
        }
      />
      </div>
    </nav>
  );
}
