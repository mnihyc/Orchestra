/** Permission-aware node actions, as a row's context menu and its "⋯".
 *
 * One list for everything that can be done with a place, written once and shown wherever the place is:
 * the right-click menu of its row in the left bar, the "⋯" beside it, and the menu in the header of its
 * screen. What the caller may do is read from what the API says they hold on that place, never from
 * a role: in the left bar an act they may not do is left out, and on the place's own screen it stays,
 * switched off, with the permission it takes and whom to ask.
 *
 * The acts that need no dialog are carried out here and shown as pending on the place while they are
 * under way; the others open their dialog, which is drawn once for the whole application.
 */
import { openPage } from '@/features/workbench/PageView';
import { useTreeNode } from '@/realtime/stores/tree';
import { actionName } from '@/ui/actions';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import {
  Archive,
  ArchiveRestore,
  ArrowRightLeft,
  BriefcasePlus,
  CheckCheck,
  CircleCheck,
  CircleDot,
  Copy,
  ExternalLink,
  FileUp,
  Focus,
  FolderInput,
  FolderPlus,
  History,
  LayoutGrid,
  ListCollapse,
  Ellipsis,
  MessageSquarePlus,
  Palette,
  Pencil,
  Plus,
  Recycle,
  Share2,
  Settings,
  Star,
  StarOff,
  Trash2,
} from 'lucide-react';
import type { ReactElement, Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { cx } from '@/ui/cx';
import { create } from 'zustand';

import { nodeQuery, unarchiveNode, updateTopic } from '@/api/nodes';
import { documentDirectoryQuery } from '@/api/documentTree';
import { useTreeActs } from '@/features/navigation/beginning';
import { openChange } from '@/features/change/target';
import { useStar } from '@/features/navigation/favorites';
import { openAccess } from '@/features/access/AccessDialog';
import { openExport, requestExportAccess } from '@/features/export/ExportHost';
import { openImport } from '@/features/people/importHostState';
import { exportEntries } from '@/features/export/menu';
import { mayExportThread } from '@/features/export/pending';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { type PlaceRef, placePath, placeUrl, useGoToPlace } from '@/features/navigation/places';
import type { Access, Node, TopicStatus, UserRef } from '@/generated/api-types';
import { abilities, canCreateIn, denial, holds } from '@/lib/permissions';
import { copyText } from '@/ui/clipboard';
import { ContextEntries, ContextMenu } from '@/ui/ContextMenu';
import { useErrorText } from '@/ui/errorText';
import { IconButton } from '@/ui/IconButton';
import { Button } from '@/ui/Button';
import { choiceName } from '@/ui/Choice';
import { Tooltip } from '@/ui/Tooltip';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { AdminActNotice, adminSpec, isAdminAct } from '@/ui/AdminAct';

/** What the list is made from: the place, what the caller holds on it, and where the list is drawn. */
export interface NodeMenuContext {
  place: PlaceRef;
  /** What the caller holds on the place; `undefined` while it has not arrived. */
  held: Pick<Access, 'permissions'> | undefined;
  starred: boolean;
  /** In a row, what the caller may not do is left out; on the place's screen it is shown switched off, with the reason. */
  where: 'row' | 'screen' | 'commands';
  /** Whom to ask for what is missing: the first owner of the place. */
  owner: UserRef | null;
  /** The org is the root of its tree: it is moved and deleted by nobody from here. */
  isRoot: boolean;
  /** Only an observed team org offers a department, never a personal or as-yet unknown org. */
  personal?: boolean;
  /** The served org address, so Settings is a real link in every org menu. */
  settingsHref?: string;
  /** For a thread: whether the caller may take its conversation away as a file, `null` while that is not known. A place is exported by whoever reads it, and does not ask. */
  exportable?: boolean | null;
  /** Whether any creation at this place is currently available for import. */
  importable?: boolean | null;
  /** A failed standing read is distinct from a denial. */
  importCheckFailed?: boolean;
  /** Whether anything of the place is unread for the caller, where the tree that is drawn knows. */
  unread?: boolean;
}

/** The acts, each as the thing to call; the list says which are offered and which are not. */
export interface NodeMenuActions {
  open: () => void;
  openInTab: () => void;
  copyLink: () => void;
  star: (starred: boolean) => void;
  newProject: () => void;
  newDepartment: () => void;
  newFolder: () => void;
  newTopic: () => void;
  newThread: () => void;
  /** Makes a document beside the place, asking its name first; left out, the menu has none. */
  newDocument?: () => void;
  rename: () => void;
  move: () => void;
  setStatus: (status: TopicStatus) => void;
  archive: () => void;
  unarchive: () => void;
  showDeleted: () => void;
  remove: () => void;
  /** Opens the wizard that switches the provider of every thread of a topic; left out, the menu has none. */
  switchThreads?: () => void;
  /** Opens the list of what is idle under a place, to stop it and make room; left out, the menu has none. */
  recycleIdle?: () => void;
  /** Opens the dialog that gives the place's icon a colour. */
  colour: () => void;
  /** Narrows the tree of the left bar to the place; left out where it cannot be, or where no tree is drawn. */
  showOnly?: () => void;
  /** Folds every branch of the tree that does not lead to the place; left out where no tree is drawn. */
  collapseOthers?: () => void;
  /** Says that the caller has read every thread and chat of the place and of what lies in it; left out where no tree is drawn. */
  markRead?: () => void;
  /** Opens the place's permission page with the field that adds people in focus; left out, the menu has no Share. */
  share?: () => void;
  /** Opens Export…: one file for a thread, an archive for a place; left out, the menu has none. */
  export?: () => void;
  /** Opens the import wizard for this place. */
  import?: () => void;
  /** Opens the form that asks for the right to take a thread away as a file. */
  requestExport?: () => void;
}

/**
 * The entries of a place's menu, in the order a person looks for them: where it leads, what can be
 * made in it, what is theirs to mark, how it is changed, and last, apart, what destroys.
 */
export function nodeMenuEntries(context: NodeMenuContext, act: NodeMenuActions, t: TFunction): MenuEntry[] {
  const { place, held, where, owner } = context;
  const own = abilities(place.kind, held);
  const can = { ...own, interact: own.interact && !place.archived, manage: own.manage && !place.archived };
  const known = held !== undefined;

  /**
   * An act that takes a permission: in a row it is left out when the caller lacks it (and while
   * nothing is known of what they hold), on a screen it is switched off and says what it takes.
   */
  const gated = (permission: string, allowed: boolean, entry: Extract<MenuEntry, { onSelect: () => void }>): MenuEntry[] => {
    if (allowed) {
      const admin = adminSpec(held, permission);
      return [isAdminAct(admin) ? { ...entry, description: <>{entry.description}<AdminActNotice spec={admin} /></> } : entry];
    }
    if (where === 'row') return [];
    return [{ ...entry, disabled: known ? place.archived ? t('places.project.archived') : denial(t, permission, place.name, owner) : t('places.menu.checking') }];
  };

  const entries: MenuEntry[] = [];
  const group = (...next: MenuEntry[]) => {
    if (next.length === 0) return;
    if (entries.length > 0) entries.push({ type: 'separator', id: `separator-${entries.length}` });
    entries.push(...next);
  };

  if (where === 'row') {
    group(
      { id: 'open', label: t('places.menu.open'), onSelect: act.open },
      { id: 'open-tab', label: t('places.menu.openInTab'), icon: <ExternalLink aria-hidden />, shortcut: 'Mod+Enter', onSelect: act.openInTab },
    );
  }

  // What the tree of the left bar does about the place: show it alone, and fold away what does not lead to it.
  const inTree: MenuEntry[] = [];
  if (act.showOnly !== undefined) inTree.push({ id: 'show-only', label: t('places.menu.showOnly'), icon: <Focus aria-hidden />, shortcut: 'Alt+Shift+ArrowRight', onSelect: act.showOnly });
  if (act.collapseOthers !== undefined) {
    inTree.push({ id: 'collapse-others', label: t('places.menu.collapseOthers'), icon: <ListCollapse aria-hidden />, onSelect: act.collapseOthers });
  }
  group(...inTree);

  // What can be made in the place.
  const make: MenuEntry[] = [];
  if (place.kind === 'org') {
    make.push(
      ...gated('org.interact', can.interact, { id: 'new-project', label: t('places.menu.newProject'), icon: <BriefcasePlus aria-hidden />, onSelect: act.newProject }),
      ...(context.personal === false ? gated('org.manage', can.manage, { id: 'new-department', label: t('places.menu.newDepartment'), icon: <LayoutGrid aria-hidden />, onSelect: act.newDepartment }) : []),
    );
  } else if (place.kind === 'project') {
    // Topics and folders have separate creation permissions.
    make.push(
      ...gated('project.create_topics', !place.archived && canCreateIn('project', held), { id: 'new-topic', label: t('places.menu.newTopic'), icon: <Plus aria-hidden />, onSelect: act.newTopic }),
      ...gated('project.interact', can.interact, { id: 'new-folder', label: t('places.menu.newFolder'), icon: <FolderPlus aria-hidden />, onSelect: act.newFolder }),
    );
  } else if (place.kind === 'topic') {
    make.push(...gated('topic.start_threads', !place.archived && canCreateIn('topic', held), { id: 'new-thread', label: t('places.menu.newThread'), icon: <MessageSquarePlus aria-hidden />, onSelect: act.newThread }));
  }
  // A document is made beside any place, by whoever may make one there, as the places and threads it holds are.
  if (act.newDocument !== undefined) {
    make.push(...gated('doc.create', held?.permissions.includes('doc.create') === true, { id: 'new-document', action: 'createDocument', label: t('places.menu.newDocument'), onSelect: act.newDocument }));
  }
  // Screens already create work in their inline starter/toolbar and documents in their own view.
  // Secondary containers stay here; the screen has its one primary New project/topic action.
  group(...(where === 'screen' ? make.filter((entry) => entry.id === 'new-department' || entry.id === 'new-folder') : make));

  // What is the person's own, and what is the place's.
  const mine: MenuEntry[] = [];
  if (act.markRead !== undefined) {
    mine.push({
      id: 'mark-read',
      label: t('places.menu.markRead'),
      icon: <CheckCheck aria-hidden />,
      onSelect: act.markRead,
      ...(context.unread === false ? { disabled: t('places.menu.nothingUnread') } : {}),
    });
  }
  mine.push(
    context.starred
      ? { id: 'unstar', label: t('places.star.unstar'), icon: <StarOff aria-hidden />, onSelect: () => act.star(false) }
      : { id: 'star', label: t('places.star.star'), icon: <Star aria-hidden />, onSelect: () => act.star(true) },
  );
  if (place.kind === 'topic') {
    mine.push(
      ...gated(
        'topic.interact',
        can.interact,
        place.status === 'done'
          ? { id: 'reopen', label: t('places.menu.reopen'), icon: <CircleDot aria-hidden />, onSelect: () => act.setStatus('open') }
          : { id: 'done', label: t('places.menu.markDone'), icon: <CircleCheck aria-hidden />, onSelect: () => act.setStatus('done') },
      ),
    );
  }
  mine.push(...gated(`${place.kind}.manage`, can.manage, { id: 'rename', label: t('places.menu.rename'), icon: <Pencil aria-hidden />, onSelect: act.rename }));
  // The colour of its icon is the place's own, the same for everybody: whoever manages it gives it one. A thread's icon says what it is doing and has none.
  if (place.kind !== 'thread') {
    mine.push(...gated(`${place.kind}.manage`, can.manage, { id: 'colour', label: t('places.menu.colour'), icon: <Palette aria-hidden />, onSelect: act.colour }));
  }
  if (place.kind === 'topic' && act.switchThreads !== undefined) {
    mine.push(...gated('topic.manage', can.manage, { id: 'switch-threads', label: t('change.menu.switchTopic'), icon: <ArrowRightLeft aria-hidden />, onSelect: act.switchThreads }));
  }
  if (place.kind !== 'thread' && act.recycleIdle !== undefined) {
    mine.push(...gated(`${place.kind}.manage`, can.manage, { id: 'recycle-idle', label: t('change.menu.recycle'), icon: <Recycle aria-hidden />, onSelect: act.recycleIdle }));
  }
  // A project and a folder go to another org or into another project; a topic to another project or folder;
  // an org that lies in another to another org. A root org goes nowhere.
  if (place.kind === 'project' || place.kind === 'topic' || (place.kind === 'org' && !context.isRoot)) {
    mine.push(...gated(`${place.kind}.manage`, can.manage, { id: 'move', label: t('places.menu.move'), icon: <FolderInput aria-hidden />, onSelect: act.move }));
  }
  group(...mine);

  const other: MenuEntry[] = [];
  if (context.settingsHref !== undefined) other.push({ id: 'settings', label: t('places.org.settings'), icon: <Settings aria-hidden />, href: context.settingsHref, onSelect: () => undefined });
  if (act.share !== undefined) other.push({ id: 'share', label: t('access.menu.share'), icon: <Share2 aria-hidden />, onSelect: act.share });
  // A place is exported by whoever reads it; a thread by whoever holds its Export row, who is told apart from a reader who does not.
  if (act.export !== undefined) {
    other.push(
      ...exportEntries({ allowed: place.kind === 'thread' ? (context.exportable ?? null) : true, owner, where: where === 'row' ? 'row' : 'screen', onExport: act.export, onRequest: act.requestExport ?? act.export, t }),
    );
  }
  if (place.kind !== 'thread' && act.import !== undefined) {
    const disabled = context.importable === true ? undefined : context.importCheckFailed === true ? t('importWizard.destination.checkFailed')
      : context.importable === null || context.importable === undefined ? t('places.menu.checking')
      : place.archived ? t('importWizard.destination.archived') : t('importWizard.destination.denied');
    other.push({ id: 'import', label: t('importWizard.mine.import'), icon: <FileUp aria-hidden />, onSelect: act.import, ...(disabled === undefined ? {} : { disabled }) });
  }
  other.push({ id: 'copy-link', label: t('places.menu.copyLink'), icon: <Copy aria-hidden />, onSelect: act.copyLink });
  if (place.kind !== 'thread' && context.held?.permissions.includes(`${place.kind}.view`) === true) {
    // The list also holds one's managed environments; each returned row has its own current guards.
    other.push({ id: 'deleted', label: t('places.menu.recentlyDeleted'), icon: <History aria-hidden />, onSelect: act.showDeleted });
  }
  group(...other);

  // What destroys goes last, apart, and in the colour of danger.
  const destroys: MenuEntry[] = [];
  destroys.push(
      ...gated(
        `${place.kind}.archive`,
        can.archive,
        place.archived
          ? { id: 'unarchive', label: t('places.menu.unarchive'), icon: <ArchiveRestore aria-hidden />, onSelect: act.unarchive }
          : { id: 'archive', label: t('places.menu.archive'), icon: <Archive aria-hidden />, onSelect: act.archive },
      ),
    );
  if (!context.isRoot) {
    destroys.push(...gated(`${place.kind}.manage`, own.manage, { id: 'delete', label: t('places.menu.delete'), icon: <Trash2 aria-hidden />, danger: true, onSelect: act.remove }));
  }
  group(...destroys);

  // Nothing is known yet of what the caller holds: the list says so rather than looking complete.
  if (where === 'row' && !known) {
    group({ type: 'label', id: 'checking', label: t('places.menu.checking') });
  }
  return entries;
}

// ---------------------------------------------------------------- the acts that need no dialog

interface Pending {
  ids: ReadonlySet<string>;
}

/** The places whose acts are under way, so that the left bar and the screens show them as pending. */
export const usePendingPlaces = create<Pending>(() => ({ ids: new Set() }));

function setPending(id: string, pending: boolean): void {
  usePendingPlaces.setState((state) => {
    const ids = new Set(state.ids);
    if (pending) ids.add(id);
    else ids.delete(id);
    return { ids };
  });
}

/** Whether an act on this place is under way. */
export function useIsPending(id: string): boolean {
  return usePendingPlaces((state) => state.ids.has(id));
}

/** The acts on a place that are made at once: its status, archiving it again, and the three that only go somewhere. */
export function useQuickActs(place: PlaceRef) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const goTo = useGoToPlace();
  const star = useStar(place);

  const refused = (title: string) => (error: unknown) =>
    toast({ tone: 'danger', title, description: describe(error).message });

  const status = useMutation({
    mutationFn: (next: TopicStatus) => updateTopic(place.id, { status: next }),
    onMutate: () => setPending(place.id, true),
    onSuccess: (topic) => {
      const back: TopicStatus = topic.status === 'done' ? 'open' : 'done';
      toast({
        tone: 'success',
        title: t(topic.status === 'done' ? 'places.toast.markedDone' : 'places.toast.reopened', { name: place.name }),
        // One Undo, on the revision this change made: a later change by anybody refuses it rather than being overwritten.
        action: {
          label: t('places.toast.undo'),
          run: () =>
            void updateTopic(place.id, { status: back, expected_revision: topic.revision })
              .catch(refused(t('places.toast.notChanged', { name: place.name }))),
        },
      });
    },
    onError: refused(t('places.toast.notChanged', { name: place.name })),
    onSettled: () => setPending(place.id, false),
  });

  const unarchive = useMutation({
    mutationFn: () => unarchiveNode(place.id),
    onMutate: () => setPending(place.id, true),
    onSuccess: () => {
      toast({ tone: 'success', title: t('places.toast.unarchived', { name: place.name }) });
    },
    onError: refused(t('places.toast.notUnarchived', { name: place.name })),
    onSettled: () => setPending(place.id, false),
  });

  const copy = () => copyText(placeUrl(place), t('common.linkCopied'));

  return {
    starred: star.starred,
    open: () => goTo(place),
    openInTab: () => { openPage(placePath(place), 'new'); },
    copyLink: () => void copy(),
    star: star.set,
    setStatus: (next: TopicStatus) => status.mutate(next),
    unarchive: () => unarchive.mutate(),
  };
}

/** The whole list of a place's menu, with its dialogs wired; `enabled` is false while nobody has asked for it to open. */
export function useNodeMenu(
  place: PlaceRef,
  options: {
    where: NodeMenuContext['where'];
    /** Command registration does not read a hidden palette's node; its cached answer still applies. */
    enabled?: boolean;
    /** The node itself, where the screen has read it already. */
    node?: Node | undefined;
  },
): MenuEntry[] {
  const { t } = useTranslation();
  const quick = useQuickActs(place);
  // A row's menu reaches the tree it stands beside; the menu of a place's own screen is about the place alone.
  const tree = useTreeActs((state) => state.acts);
  const inTree = options.where === 'row' ? tree : null;
  const query = useQuery({ ...nodeQuery(place.id), enabled: options.node === undefined && options.enabled !== false });
  const node = options.node ?? query.data;
  const cache = useQueryClient();
  const directImport = node !== undefined && !node.archived && place.kind !== 'thread'
    && (holds(node.access, `${place.kind}.interact`) || place.kind === 'org' && holds(node.access, 'org.manage') || holds(node.access, 'chat.interact'));
  const checkDocuments = node !== undefined && !node.archived && place.kind !== 'thread' && !directImport && holds(node.access, 'doc.create');
  const directory = useQuery({ ...documentDirectoryQuery({ node: place.id, at: { kind: 'node_docs', id: place.id } }, cache), enabled: checkDocuments && options.enabled !== false });
  const importable = node === undefined ? null : node.archived ? false : directImport ? true : checkDocuments ? directory.isPending ? null : directory.data?.directory.can_create === true : false;

  const isRoot = node?.kind === 'org' ? node.parent_id === null : false;
  const importPath = node?.path.map((part) => part.redacted ? t('ui.nameUnknown') : part.name ?? t('ui.nameUnknown')).join(' / ');
  const { starred, ...quickActs } = quick;
  const act: NodeMenuActions = {
    ...quickActs,
    newProject: () => openNodeDialog({ type: 'new-project', parent: place }),
    newDepartment: () => openNodeDialog({ type: 'new-department', parent: place }),
    newFolder: () => openNodeDialog({ type: 'new-folder', parent: place }),
    newTopic: () => openNodeDialog({ type: 'new-topic', parent: place }),
    newDocument: () => openNodeDialog({ type: 'new-document', parent: place }),
    newThread: () => openNodeDialog({ type: 'new-thread', parent: place }),
    rename: () => openNodeDialog({ type: 'rename', node: place }),
    move: () => openNodeDialog({ type: 'move', node: place }),
    archive: () => openNodeDialog({ type: 'archive', node: place }),
    showDeleted: () => openNodeDialog({ type: 'deleted', node: place }),
    remove: () => openNodeDialog({ type: 'delete', node: place }),
    ...(place.kind === 'topic' ? { switchThreads: () => openChange({ kind: 'topic', id: place.id, name: place.name }, { kind: 'switch' }) } : {}),
    ...(place.kind === 'thread' ? {} : { recycleIdle: () => openChange({ kind: 'node', id: place.id, name: place.name }, { kind: 'recycle' }) }),
    colour: () => openNodeDialog({ type: 'colour', node: place }),
    ...(inTree?.canShowOnly(place.id) === true ? { showOnly: () => inTree.showOnly(place.id) } : {}),
    // The org the tree begins under is on the way to every row: there are no others to collapse beside it.
    ...(inTree !== null && place.parentId !== null ? { collapseOthers: () => inTree.collapseOthers(place.id) } : {}),
    ...(inTree !== null ? { markRead: () => inTree.markRead(place.id) } : {}),
    share: () => openAccess({ kind: place.kind, id: place.id, name: place.name }),
    export: () => openExport({ kind: place.kind, id: place.id, name: place.name }),
    ...(place.kind === 'thread' ? {} : { import: () => openImport({ id: place.id, label: importPath === undefined || importPath === '' ? place.name : importPath }) }),
    requestExport: () => requestExportAccess({ kind: place.kind, id: place.id, name: place.name, owner: node?.owner ?? null }),
  };
  return nodeMenuEntries(
    {
      place,
      held: node?.access,
      starred,
      where: options.where,
      owner: node?.owner ?? null,
      isRoot,
      personal: node?.kind === 'org' ? node.org_kind === 'personal' : undefined,
      ...(node?.kind === 'org' ? { settingsHref: `/org/${encodeURIComponent(node.address)}/settings/general` } : {}),
      exportable: place.kind === 'thread' ? (node?.kind === 'thread' ? mayExportThread(node) : null) : undefined,
      importable,
      importCheckFailed: checkDocuments && directory.isError,
      ...(inTree !== null ? { unread: inTree.hasUnread(place.id) } : {}),
    },
    act,
    t,
  );
}

export interface NodeMenuButtonProps {
  place: PlaceRef;
  /** In a row what the caller may not do is left out; on the place's screen it is shown switched off. */
  where?: 'row' | 'screen';
  /** The node itself, where the screen has read it already. */
  node?: Node | undefined;
  className?: string;
  /** Makes the button take no place in the order of the Tab key: a row of a tree is moved along by its arrow keys. */
  skipTab?: boolean;
  /** The containing view adds layout choices to this same lazy native menu. */
  extraEntries?: MenuEntry[];
  menuLabel?: string;
  /** A menu can open a dialog whose stable keyboard return point is this trigger. */
  triggerRef?: Ref<HTMLButtonElement>;
}

/** The list of a place's menu, made only while the menu is open. */
function Entries({ place, where, node, only, extraEntries }: { place: PlaceRef; where: 'row' | 'screen'; node?: Node | undefined; only?: 'make'; extraEntries?: MenuEntry[] }) {
  const { t } = useTranslation();
  const entries = useNodeMenu(place, { where, node });
  const visible = entries;
  if (only !== 'make') return <DropdownEntries entries={extraEntries === undefined || extraEntries.length === 0 ? visible : where === 'screen' && place.kind === 'org' ? [...extraEntries, { type: 'separator', id: 'place-details' }, ...visible] : [...visible, { type: 'separator', id: 'view-layout' }, ...extraEntries]} />;
  // The list of what can be made here: the entries that make something, and a word where there are none.
  const made = entries.filter((entry) => 'id' in entry && entry.id.startsWith('new-'));
  const checking = entries.some((entry) => 'id' in entry && entry.id === 'checking');
  return (
    <DropdownEntries
      entries={
        made.length > 0
          ? made
          : [{ type: 'label', id: 'none', label: checking ? t('places.menu.checking') : t('places.menu.nothingToMake') }]
      }
    />
  );
}

/** The "⋯" of a place: the same list as its right-click menu, for touch and for whoever does not right-click. */
export function NodeMenuButton({ place, where = 'row', node, className, skipTab = false, extraEntries, menuLabel, triggerRef }: NodeMenuButtonProps) {
  const parent = useTreeNode(place.parentId);
  const menuName = menuLabel ?? actionName('actions', where === 'row' && parent !== null ? `${place.name} · ${parent.name}` : place.name);
  return (
    <Menu
      label={menuName}
      align="end"
      content={<Entries place={place} where={where} node={node} extraEntries={extraEntries} />}
      trigger={
        <IconButton
          ref={triggerRef}
          size="sm"
          action="actions" label={menuName}
          icon={<Ellipsis aria-hidden />}
          className={className}
          {...(skipTab ? { tabIndex: -1 } : {})}
        />
      }
    />
  );
}

/** A page toolbar wraps its named acts; only the compact row/context menu needs an overflow trigger. */
export function NodeActionButtons({ entries, label, className }: { entries: readonly MenuEntry[]; label: string; className?: string | undefined }) {
  return <div role="group" aria-label={label} data-node-screen-actions className={cx('flex flex-wrap items-center gap-1', className)}>
    {entries.flatMap((entry) => {
      if (entry.type !== undefined && entry.type !== 'item') return [];
      const name = choiceName(entry);
      const off = typeof entry.disabled === 'string';
      return [<Tooltip key={entry.id} content={typeof entry.disabled === 'string' ? entry.disabled : entry.description}>
        <Button size="sm" variant={entry.danger ? 'danger-ghost' : 'ghost'} icon={entry.icon} blocked={off} asChild={entry.href !== undefined && !off} onClick={entry.onSelect}>
          {entry.href !== undefined && !off ? <a href={entry.href}>{name}</a> : name}
        </Button>
      </Tooltip>];
    })}
  </div>;
}


/** The "+" of an org or a project: what can be made in it, for those who may make it. */
export function NodeMakeButton({ place, node, className, skipTab = false }: { place: PlaceRef; node?: Node | undefined; className?: string; skipTab?: boolean }) {
  const { t } = useTranslation();
  return (
    <Menu
      label={t('places.menu.new', { name: place.name })}
      align="end"
      content={<Entries place={place} where="row" node={node} only="make" />}
      trigger={
        <IconButton
          size="sm"
          label={t('places.menu.new', { name: place.name })}
          icon={<Plus aria-hidden />}
          className={className}
          {...(skipTab ? { tabIndex: -1 } : {})}
        />
      }
    />
  );
}

/** Gives whatever it is put round the right-click menu of a place: the same list as its "⋯". */
export function NodeContextMenu({ place, children, extraEntries }: { place: PlaceRef; children: ReactElement; extraEntries?: MenuEntry[] | undefined }) {
  const { t } = useTranslation();
  return (
    <ContextMenu label={t('places.menu.of', { name: place.name })} content={<ContextList place={place} extraEntries={extraEntries} />}>
      {children}
    </ContextMenu>
  );
}

function ContextList({ place, extraEntries = [] }: { place: PlaceRef; extraEntries?: MenuEntry[] | undefined }) {
  const entries = useNodeMenu(place, { where: 'row' });
  return <ContextEntries entries={[...entries, ...extraEntries]} />;
}
