/** The commands that hold everywhere or follow the screen in view: going to a screen, making things, how the page looks, the workbench, and the acts on the place being looked at.
 *
 * They are registered once, by the shell, and are the commands of the finder's `>` mode. A command that
 * leads to a screen is registered by the route the screen has in the table of routes, and is listed only
 * where the router has that route, so that a screen joined to the application brings its commands with
 * it and one that is not there offers none. The acts on the place in view are the entries of that place's
 * own menu, made commands: one list of acts, written once, with what the caller may not do switched off and
 * why.
 */
import { screenBuilt } from '@/shell/availability';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import { useMutation } from '@tanstack/react-query';
import { useRouter } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import {
  Activity,
  Archive,
  ArchiveRestore,
  Bell,
  BookOpen,
  Boxes,
  Building,
  CircleCheck,
  CircleDot,
  CircleQuestionMark,
  Copy,
  Cpu,
  Contrast,
  Gauge,
  HardDrive,
  History,
  Inbox,
  Keyboard,
  KeyRound,
  Laptop,
  LayoutGrid,
  LayoutPanelLeft,
  Link2,
  ListTree,
  LogOut,
  type LucideIcon,
  MessagesSquare,
  PanelBottom,
  PanelLeft,
  PanelRight,
  Pencil,
  Plus,
  PlugZap,
  RotateCcw,
  Rows3,
  Search,
  Server,
  Settings,
  ShieldCheck,
  ShieldUser,
  Share2,
  SlidersHorizontal,
  Star,
  StarOff,
  Trash2,
  UserRound,
  UsersRound,
  Wrench,
  Bot,
  FolderInput,
  FolderPlus,
  MessageSquarePlus,
  Languages,
  Rows4,
  GraduationCap,
} from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { meKey, meQuery, updateMe } from '@/api/auth';
import { adminModeKey } from '@/api/adminMode';
import { queryClient } from '@/api/query';
import { orgsQuery } from '@/api/nodes';
import { type Command, useRegisterCommands } from '@/features/finder/commands';
import { CreationHost, openCreation } from '@/features/finder/Creation';
import { useMenuCommands } from '@/features/finder/menuCommands';
import { useFinder } from '@/features/finder/store';
import { settingsDestinations, type SettingsPlace } from '@/features/finder/settingsCommands';
import { PERSONAL_PAGES, personalTitle } from '@/features/settings/navigation';
import { LISTED } from '@/features/admin/sections';
import { useAdminMode, useAdminModeWrite } from '@/features/access/AdminMode';
import { pathOf } from '@/features/finder/destinations';
import { useHelpOpener } from '@/features/help/open';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { type NodeMenuContext, useNodeMenu } from '@/features/navigation/NodeMenu';
import { type PlaceRef, placeOfRow } from '@/features/navigation/places';
import { ARRANGEMENTS, arrangementOf } from '@/features/workbench/Arrangements';
import { useShape } from '@/features/workbench/Narrow';
import { useWorkbench } from '@/features/workbench/store';
import type { AdminMode, Density, Me, MeUpdateBody, Theme } from '@/generated/api-types';
import { LOCALES, type TranslationKey } from '@/i18n';
import { adoptAppearance, useAppearance } from '@/lib/appearance';
import { useNodePath, useTreeNode } from '@/realtime/stores/tree';
import { leave } from '@/routes/-access';
import { type KeyActionId, keysAct } from '@/shell/keyActions';
import { requestKeyList, useKeyChoices } from '@/shell/keys';
import { useSelectedOrg } from '@/shell/org';
import { useCurrentHeading } from '@/shell/screen';
import { useQuery } from '@tanstack/react-query';
import { useErrorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

/** A word list written in the catalogue as one text, split into the words that find a command. */
const words = (t: TFunction, key: TranslationKey): string[] => t(key).split(/\s+/).filter((word) => word !== '');

interface Where {
  to: string;
  /** The screen in words: a key of `screens.<name>.title`. */
  screen: 'inbox' | 'approvals' | 'shared' | 'usage' | 'help' | 'search' | 'providers' | 'agents' | 'environments' | 'hosts' | 'machines' | 'templates' | 'mcp' | 'mounts' | 'skills' | 'profile' | 'security' | 'notifications' | 'settingsUsage' | 'tokens' | 'interface';
  section?: 'resources' | 'settings';
  icon: LucideIcon;
  /** The keyboard action that goes there too, whose keys the command shows. */
  action?: KeyActionId;
}

/** The screens that hold for a person wherever they are, by the route each has in the table of routes. */
const SCREENS: readonly Where[] = [
  { to: '/inbox', screen: 'inbox', icon: Inbox, action: 'inbox' },
  { to: '/approvals', screen: 'approvals', icon: ShieldCheck, action: 'approvals' },
  { to: '/shared', screen: 'shared', icon: Share2 },
  { to: '/search', screen: 'search', icon: Search, action: 'search' },
  { to: '/usage', screen: 'usage', icon: Activity },
  { to: '/help', screen: 'help', icon: BookOpen },
  { to: '/resources/providers', screen: 'providers', section: 'resources', icon: PlugZap },
  { to: '/resources/agents', screen: 'agents', section: 'resources', icon: Bot },
  { to: '/resources/environments', screen: 'environments', section: 'resources', icon: Boxes },
  { to: '/resources/hosts', screen: 'hosts', section: 'resources', icon: Server },
  { to: '/resources/machines', screen: 'machines', section: 'resources', icon: Laptop },
  { to: '/resources/templates', screen: 'templates', section: 'resources', icon: Cpu },
  { to: '/resources/mcp', screen: 'mcp', section: 'resources', icon: Wrench },
  { to: '/resources/mounts', screen: 'mounts', section: 'resources', icon: HardDrive },
  { to: '/resources/skills', screen: 'skills', section: 'resources', icon: GraduationCap },
  { to: '/settings/profile', screen: 'profile', section: 'settings', icon: UserRound },
  { to: '/settings/security', screen: 'security', section: 'settings', icon: KeyRound },
  { to: '/settings/notifications', screen: 'notifications', section: 'settings', icon: Bell },
  { to: '/settings/usage', screen: 'settingsUsage', section: 'settings', icon: Gauge },
  { to: '/settings/tokens', screen: 'tokens', section: 'settings', icon: KeyRound },
  { to: '/settings/interface', screen: 'interface', section: 'settings', icon: SlidersHorizontal },
];

const PLACE_ICONS: Record<string, LucideIcon> = {
  'new-project': FolderPlus,
  'new-department': LayoutGrid,
  'new-topic': Plus,
  'new-thread': MessageSquarePlus,
  star: Star,
  unstar: StarOff,
  done: CircleCheck,
  reopen: CircleDot,
  rename: Pencil,
  move: FolderInput,
  'copy-link': Copy,
  deleted: History,
  archive: Archive,
  unarchive: ArchiveRestore,
  delete: Trash2,
};

/** The entries of a place's menu that do something: the others are separators, labels and switches. */
const SKIPPED = new Set(['open', 'open-tab', 'checking', 'new-thread', 'new-document']);

/** The commands that hold wherever the reader is. */
export function useGlobalCommands(): void {
  const { t } = useTranslation();
  const router = useRouter();
  const me = useQuery(meQuery).data;
  const mode = useAdminMode();
  const { setMode, changing: changingMode } = useAdminModeWrite();
  const heading = useCurrentHeading();
  const context = useTreeNode(heading?.nodeId);
  const { path } = useNodePath(heading?.nodeId);
  const startAt = (heading?.path ?? path).findLast((part) => part.kind === 'topic' || part.kind === 'project')?.id ?? null;
  const orgId = useSelectedOrg((state) => state.orgId);
  const documentAt = heading?.nodeId ?? orgId;
  const orgs = useQuery(orgsQuery).data;
  const org = orgs?.find((candidate) => candidate.id === orgId);
  const screen = useWorkbench((state) => state.screen);
  const choose = useWorkbench((state) => state.choose);
  const reset = useWorkbench((state) => state.reset);
  const toggle = useWorkbench((state) => state.toggle);
  const shape = useShape();
  const beside = shape === 'wide';
  const openHelp = useHelpOpener();
  const saveAppearance = useAppearanceSaver();
  // A command shows the keys its action has now: the person's own, or none.
  const choices = useKeyChoices();
  const system = me?.is_system_admin === true;
  const slug = org?.slug ?? null;
  const exists = (to: string): boolean => Object.hasOwn(router.routesByPath, to);

  const commands = useMemo<Command[]>(() => {
    const list: Command[] = [];
    const keysOf = (action: KeyActionId | undefined) => (action === undefined ? undefined : keysAct(action, choices) ?? undefined);
    /** A command that leads to a screen: by the route it has in the table of routes, and offered only where the router has the route. */
    const leadsTo = (id: string, pattern: string, address: string, title: Command['title'], icon: LucideIcon, shortcut?: string) =>
      list.push({
        id,
        kind: 'go',
        title,
        keywords: (t2) => words(t2, 'commands.words.go'),
        icon,
        ...(shortcut === undefined ? {} : { shortcut }),
        path: address,
        run: (how) => (how === 'tab' ? window.open(address, '_blank', 'noopener') : router.history.push(address)),
        available: () => exists(pattern) && screenBuilt(address),
      });

    for (const where of SCREENS.filter((entry) => entry.section !== 'settings')) {
      const name = (t2: TFunction) =>
        where.section === undefined ? t2(`screens.${where.screen}.title`) : `${t2(`screens.sections.${where.section}`)} › ${t2(`screens.${where.screen}.title`)}`;
      leadsTo(`go.${where.to}`, where.to, where.to, (t2) => t2('commands.goTo', { where: name(t2) }), where.icon, keysOf(where.action));
    }
    for (const page of PERSONAL_PAGES) {
      if (SCREENS.some((entry) => entry.section !== 'settings' && entry.to === page.to)) continue;
      leadsTo(`go.${page.to}`, page.to, page.to, (t2) => t2('commands.personalSection', { section: personalTitle(page.key, t2) }), page.Icon);
    }

    // The complete hierarchy is a main-area page on a phone as on the desktop.
    for (const view of ['performance', 'requests'] as const) {
      const address = pathOf({ to: '/usage', search: { view } });
      list.push({ id: `go.usage.${view}`, kind: 'go', title: (t2) => t2(view === 'performance' ? 'usagePerf.finderPerformance' : 'usagePerf.finderRequests'), keywords: (t2) => words(t2, 'usagePerf.finderWords'), icon: Gauge, path: address, run: (how) => how === 'tab' ? window.open(address, '_blank', 'noopener') : router.history.push(address), available: () => exists('/usage') });
    }
    leadsTo('go.everything', '/everything', '/everything', (t2) => t2('commands.goTo', { where: t2('everything.title') }), ListTree);

    // The messages, the home and the settings of the org in view.
    if (slug !== null) {
      leadsTo('go.messages', '/org/$slug/messages', pathOf({ to: '/org/$slug/messages', params: { slug } }), (t2) => t2('commands.goTo', { where: t2('screens.messages.title') }), MessagesSquare, keysOf('messages'));
      if (org !== undefined && !(context?.kind === 'org' && context.id === org.id)) {
        for (const section of settingsDestinations({ ...org, kind: 'org' }, t)) leadsTo(`go.org-settings.${section.id}`, section.pattern, section.path, () => section.title, Settings);
      }
      leadsTo('go.org', '/org/$slug', pathOf({ to: '/org/$slug', params: { slug } }), () => t('commands.goTo', { where: org?.name ?? '' }), Building);
    }

    // Management is for the administrators of the system, who expect it; nobody else is offered it.
    if (system) for (const section of LISTED) {
      leadsTo(`go.admin.${section.id}`, '/admin/$section', pathOf({ to: '/admin/$section', params: { section: section.id } }),
        (t2) => t2('commands.settingsSection', { scope: t2('screens.admin.title'), section: t2(`admin.sections.${section.id}.title`) }), section.icon);
    }
    if (mode.data?.available === true) {
      const enabled = !mode.data.as_admin;
      list.push({ id: `account.admin.${enabled ? 'on' : 'off'}`, kind: 'account', icon: ShieldUser,
        title: (t2) => t2(enabled ? 'commands.adminMode.on' : 'commands.adminMode.off'), keywords: (t2) => words(t2, 'commands.words.admin'),
        available: () => {
          const current = queryClient.getQueryData<AdminMode>(adminModeKey);
          if (current?.available !== true || current.as_admin === enabled) return false;
          return queryClient.isMutating({ mutationKey: ['admin-mode'] }) > 0 ? { disabled: t('commands.adminMode.busy') } : true;
        },
        run: () => setMode(enabled),
      });
    }

    // Making.
    list.push(
      { id: 'create.thread', kind: 'create', title: (t2) => t2('commands.create.thread'), keywords: (t2) => words(t2, 'commands.words.thread'), icon: MessageSquarePlus, run: () => openCreation('thread', startAt) },
      { id: 'create.document', kind: 'create', title: (t2) => t2('commands.create.document'), keywords: (t2) => words(t2, 'commands.words.document'), icon: Pencil, run: () => openCreation('document', documentAt) },
      { id: 'create.org', kind: 'create', title: (t2) => t2('shell.rail.newOrg'), keywords: (t2) => words(t2, 'commands.words.make'), icon: UsersRound, run: () => openNodeDialog({ type: 'new-org' }) },
      { id: 'create.join', kind: 'create', title: (t2) => t2('shell.rail.join'), keywords: (t2) => words(t2, 'commands.words.join'), icon: Link2, run: () => openNodeDialog({ type: 'join' }) },
    );

    // How the page looks.
    const theme = (value: Theme, label: TranslationKey, icon: LucideIcon): Command => ({
      id: `appearance.theme.${value}`,
      kind: 'appearance',
      title: (t2) => t2('commands.appearance.theme', { name: t2(label) }),
      keywords: (t2) => words(t2, 'commands.words.appearance'),
      icon,
      run: () => saveAppearance({ theme: value }),
    });
    list.push(
      {
        id: 'appearance.toggle',
        kind: 'appearance',
        title: (t2) => t2('commands.appearance.toggle'),
        keywords: (t2) => words(t2, 'commands.words.appearance'),
        icon: Contrast,
        run: () => saveAppearance({ theme: useAppearance.getState().resolvedTheme === 'dark' ? 'light' : 'dark' }),
      },
      theme('light', 'preferences.themeLight', Contrast),
      theme('dark', 'preferences.themeDark', Contrast),
      theme('system', 'preferences.themeSystem', Contrast),
    );
    const density = (value: Density, label: TranslationKey, icon: LucideIcon): Command => ({
      id: `appearance.density.${value}`,
      kind: 'appearance',
      title: (t2) => t2('commands.appearance.density', { name: t2(label) }),
      keywords: (t2) => words(t2, 'commands.words.density'),
      icon,
      run: () => saveAppearance({ density: value }),
    });
    list.push(density('comfortable', 'preferences.densityComfortable', Rows3), density('compact', 'preferences.densityCompact', Rows4));
    for (const entry of LOCALES) {
      list.push({
        id: `appearance.language.${entry.locale}`,
        kind: 'appearance',
        title: (t2) => t2('commands.appearance.language', { name: entry.name }),
        keywords: (t2) => words(t2, 'commands.words.language'),
        icon: Languages,
        run: () => saveAppearance({ locale: entry.locale }),
      });
    }

    // The workbench: its bars, its arrangements, and the views of the screen.
    const bar = (id: 'left' | 'right' | 'bottom', icon: LucideIcon, action: KeyActionId): Command => ({
      id: `view.bar.${id}`,
      kind: 'view',
      title: (t2) => t2(`workbench.bars.toggle.${id}`),
      keywords: (t2) => words(t2, 'commands.words.bar'),
      icon,
      ...(keysOf(action) === undefined ? {} : { shortcut: keysOf(action) }),
      run: () => toggle(id, beside),
    });
    list.push(bar('left', PanelLeft, 'left-bar'), bar('right', PanelRight, 'right-bar'), bar('bottom', PanelBottom, 'bottom'));
    for (const arrangement of ARRANGEMENTS[screen]) {
      list.push({
        id: `view.arrangement.${arrangement.name}`,
        kind: 'view',
        title: (t2) => t2(arrangement.title),
        keywords: (t2) => words(t2, 'commands.words.arrangement'),
        about: (t2) => t2(arrangement.about),
        icon: LayoutPanelLeft,
        run: () => choose(arrangementOf(screen, arrangement.name).name),
      });
    }
    list.push({ id: 'view.arrangement.reset', kind: 'view', title: (t2) => t2('commands.arrangement.reset'), keywords: (t2) => words(t2, 'commands.words.arrangement'), icon: RotateCcw, run: () => reset() });

    // Help, the keys, and the way out.
    list.push(
      {
        id: 'help.screen',
        kind: 'go',
        title: (t2) => t2('finder.offers.help'),
        keywords: (t2) => words(t2, 'commands.words.help'),
        icon: CircleQuestionMark,
        ...(keysOf('help') === undefined ? {} : { shortcut: keysOf('help') }),
        run: () => openHelp(),
      },
      // The list of keys is found here also when `?` is switched off or moved.
      { id: 'account.keys', kind: 'account', title: (t2) => t2('shell.shortcuts.title'), keywords: (t2) => words(t2, 'commands.words.keys'), icon: Keyboard, ...(keysOf('list') === undefined ? {} : { shortcut: keysOf('list') }), run: () => requestKeyList() },
      { id: 'account.sign-out', kind: 'account', title: (t2) => t2('shell.account.signOut'), keywords: (t2) => words(t2, 'commands.words.signOut'), icon: LogOut, run: () => void leave() },
    );
    return list;
    // The list follows the org in view, whether the person administers the system, the screen and the room there is; the rest it reads when it is run.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug, org, context?.kind, context?.id, system, screen, beside, router, t, choices, startAt, documentAt, mode.data?.available, mode.data?.as_admin, changingMode]);

  useRegisterCommands(commands);
}

/** Saves a preference as the account menu does: it shows at once, and is put right if the server refuses. */
function useAppearanceSaver(): (change: Partial<Pick<Me, 'theme' | 'density' | 'locale'>>) => void {
  const { t } = useTranslation();
  const describe = useErrorText();
  const set = useAppearance((state) => state.set);
  const save = useMutation({
    mutationFn: (change: MeUpdateBody) => updateMe(change),
    onError: (error) => {
      const me = queryClient.getQueryData<Me>(meKey);
      if (me !== undefined) adoptAppearance(me);
      toast({ tone: 'danger', title: t('shell.account.notSaved'), description: describe(error).message });
    },
  });
  return (change) => {
    set(change);
    save.mutate(change);
  };
}

/** What the entries of the menu of the place in view say, as commands: the place's own acts, with why not where it is not for the reader. */
export function PlaceCommands({ place, settingsPlace }: { place: PlaceRef; settingsPlace?: SettingsPlace | null }) {
  const router = useRouter();
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const name = label(place) ?? place.name;
  const finderOpen = useFinder((state) => state.open);
  const entries = useNodeMenu(place, { where: 'commands', enabled: finderOpen });
  useMenuCommands(entries, { prefix: 'place', name, icons: PLACE_ICONS, skip: SKIPPED });
  const org = useQuery({ ...orgsQuery, enabled: false }).data?.find((entry) => entry.id === place.id);
  const at: SettingsPlace | null = settingsPlace ?? (place.kind === 'thread' ? null : { ...place, kind: place.kind, name, flat: org?.flat });
  const scopeId = at?.id; const scopeKind = at?.kind; const scopeName = at?.name; const flat = at?.flat; const address = at?.address;

  // Where the settings of the place in view are, by the route its kind has in the table of routes.
  const settings = useMemo<Command[]>(() => {
    if (scopeId === undefined || scopeKind === undefined || scopeName === undefined) return [];
    return settingsDestinations({ id: scopeId, kind: scopeKind, name: scopeName, flat, address }, t).map((target): Command => ({
        id: `go.place-settings.${target.id}`,
        kind: 'go', title: () => target.title,
        keywords: (t) => words(t, 'commands.words.go'),
        icon: Settings,
        path: target.path,
        run: (how) => (how === 'tab' ? window.open(target.path, '_blank', 'noopener') : router.history.push(target.path)),
        available: () => Object.hasOwn(router.routesByPath, target.pattern),
      }));
  }, [scopeId, scopeKind, scopeName, flat, address, router, t]);
  useRegisterCommands(settings);
  return null;
}

/** Stands once in the shell: registers the commands that hold everywhere and those of the place in view. */
export function ShellCommands() {
  useGlobalCommands();
  const heading = useCurrentHeading();
  const nodeId = heading?.nodeId;
  const { path } = useNodePath(nodeId);
  const scope = (heading?.path ?? path).findLast((part) => part.kind === 'project' || part.kind === 'topic');
  const settingsPlace: SettingsPlace | null = scope?.name === null || scope === undefined || (scope.kind !== 'project' && scope.kind !== 'topic') ? null : { id: scope.id, name: scope.name, kind: scope.kind };
  const row = useTreeNode(nodeId);
  const place = useMemo(() => (row === null ? null : placeOfRow(row)), [row]);
  return <><CreationHost />{place !== null && <PlaceCommands key={place.id} place={place} settingsPlace={place.kind === 'thread' ? settingsPlace : null} />}</>;
}

export type { NodeMenuContext };
