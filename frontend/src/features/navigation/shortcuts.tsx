/** Shortcuts: a pointer to a document, a chat, a thread or a place, kept in a place or in a person's own list (Starred).
 *
 * A shortcut shows its target's current name and icon with a small arrow mark and opens the target; it follows the
 * target through moves and renames, since it points by identity. It grants nothing: a target its reader may not open
 * is "Unavailable" with nothing more said of it, and one that was deleted is "Deleted" until it is restored. Its acts
 * move or remove the shortcut and never touch the target; one is dragged onto another place, or onto Starred, to be
 * moved there. "Add shortcut…" on a document, a chat, a thread or a place keeps one in a place the person chooses.
 */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { ArrowUpRight, CornerUpRight, Ellipsis, ExternalLink, FolderInput, MessageSquare, Star, StarOff, Trash2 } from 'lucide-react';
import { type DragEvent, type ReactElement, type ReactNode, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { favoritesKey, starItem, unstarItem } from '@/api/personal';
import { moveShortcut, removeShortcut, type Shortcut, shortcutsOfPlaceQuery } from '@/api/shortcuts';
import { isPlainClick, useOpenPath } from '@/features/chat/open';
import { useIsStarred } from '@/features/navigation/favorites';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import type { NameKind, PersonalTarget } from '@/generated/api-types';
import { linkTo } from '@/lib/links';
import { useTreeNode } from '@/realtime/stores/tree';
import { PathLink as RoutedPathLink } from '@/shell/TrailLink';
import { ContextEntries, ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';
import { NodeIcon, type NodeIconKind } from '@/ui/NodeIcon';
import { useErrorText } from '@/ui/errorText';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

/** What a shortcut is asked for: the thing it points to, by kind and identity, and its name to say it by. */
export interface ShortcutTarget extends PersonalTarget {
  name: string;
}

/** The kinds a shortcut is kept to in a place, and that "Add shortcut…" is offered on. */
export const SHORTCUT_KINDS: readonly NameKind[] = ['document', 'chat', 'thread', 'project', 'topic'];

/** What a shortcut row is carried as when it is dragged: the shortcut, to be put where it is dropped. */
export const DRAGGED_SHORTCUT = 'application/x-orch-shortcut';

/** What a dragged shortcut carries. */
interface Carried {
  id: string;
  name: string;
  /** The place it is kept in; `null` in Starred. */
  from: string | null;
}

/** The glyph of a target's kind: a project that lies in a project is a folder. */
function iconKind(shortcut: Shortcut): NodeIconKind | 'chat' {
  const kind = shortcut.target.kind;
  if (kind === 'project') return shortcut.item?.path.at(-1)?.kind === 'project' ? 'folder' : 'project';
  if (kind === 'org' || kind === 'topic' || kind === 'thread' || kind === 'document') return kind;
  return 'chat';
}

/** A shortcut's icon: its target's glyph in its colour, with the small arrow that says it is a shortcut. */
export function ShortcutIcon({ shortcut, arrow = true }: { shortcut: Shortcut; arrow?: boolean }) {
  const kind = iconKind(shortcut);
  const open = shortcut.state === 'open';
  return (
    <span className={cx('relative flex size-4 shrink-0 items-center justify-center', !open && 'opacity-60')} data-shortcut-icon={kind}>
      {kind === 'chat' ? (
        <MessageSquare aria-hidden className="size-4 text-text-subtle" />
      ) : (
        <NodeIcon kind={kind} color={shortcut.item?.color ?? null} own={shortcut.item?.custom_color ?? null} />
      )}
      {arrow && (
        <span aria-hidden className="absolute -bottom-0.5 -left-0.5 flex size-2.5 items-center justify-center rounded-xs bg-bg text-text ring-1 ring-border">
          <ArrowUpRight size={8} strokeWidth={3} />
        </span>
      )}
    </span>
  );
}

/** The word for what a shortcut points to, as a screen reader is told it. */
function kindLabel(t: TFunction, shortcut: Shortcut): string {
  const kind = iconKind(shortcut);
  return kind === 'chat' ? t('navigation.chat.name') : t(`navigation.row.${kind}` as 'navigation.row.document');
}

/** What a shortcut is called: its target's current name, or what is said in its place where there is none to give. */
export function shortcutName(t: TFunction, shortcut: Shortcut): string {
  const name = shortcut.item?.name;
  if (shortcut.state === 'open' && name !== undefined && name !== null) return name;
  return shortcut.state === 'deleted' ? t('shortcuts.state.deleted') : t('shortcuts.state.unavailable');
}

/** Where a shortcut leads: its target's own screen, the same address Copy link gives; none while it does not open. */
export function shortcutPath(shortcut: Shortcut): string | null {
  if (shortcut.state !== 'open') return null;
  const { kind, id } = shortcut.target;
  if (kind !== 'org' && kind !== 'project' && kind !== 'topic' && kind !== 'thread' && kind !== 'document' && kind !== 'chat') return null;
  return new URL(linkTo({ kind, id })).pathname;
}

/**
 * Moves a shortcut to a place, or with `null` into Starred, and says where it went; a refusal is said and changes
 * nothing. Every row of the tree takes a dropped shortcut, so this keeps nothing of its own for a row: the move is
 * asked for only when one is dropped, and the lists it changes are read again as after any act.
 */
export function useMoveShortcut(): (carried: { id: string; name: string }, to: { id: string | null; name: string }) => void {
  const { t } = useTranslation();
  const describe = useErrorText();
  return (carried, to) => {
    moveShortcut(carried.id, to.id).then(
      () => toast({ tone: 'success', title: t('shortcuts.moved', { name: carried.name, place: to.name }) }),
      (error: unknown) => toast({ tone: 'danger', title: t('shortcuts.notMoved', { name: carried.name }), description: describe(error).message }),
    );
  };
}

/** What dropping a dragged shortcut on a place, or on Starred (`to.id` null), does: it is moved there at once. */
export function useShortcutDrop(to: { id: string | null; name: string } | null) {
  const move = useMoveShortcut();
  return {
    onDragOver: (event: DragEvent) => {
      if (to === null || !event.dataTransfer.types.includes(DRAGGED_SHORTCUT)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
    },
    onDrop: (event: DragEvent) => {
      const carried = event.dataTransfer.getData(DRAGGED_SHORTCUT);
      if (to === null || carried === '') return;
      event.preventDefault();
      event.stopPropagation();
      const shortcut = JSON.parse(carried) as Carried;
      if (shortcut.from === to.id) return;
      move(shortcut, to);
    },
  };
}

/** The acts of a shortcut: open its target, move or remove the shortcut. Nothing here touches the target. */
export function useShortcutEntries(shortcut: Shortcut): MenuEntry[] {
  const { t } = useTranslation();
  const open = useOpenPath();
  const describe = useErrorText();
  const queryClient = useQueryClient();
  const name = shortcutName(t, shortcut);
  const remove = useMutation({
    mutationFn: () => removeShortcut(shortcut.id),
    onSuccess: () => toast({ tone: 'success', title: t('shortcuts.removed', { name }) }),
    onError: (error) => {
      void queryClient.invalidateQueries({ queryKey: favoritesKey });
      toast({ tone: 'danger', title: t('shortcuts.notRemoved', { name }), description: describe(error).message });
    },
  });
  const path = shortcutPath(shortcut);
  const entries: MenuEntry[] = [];
  if (path !== null) {
    entries.push(
      { id: 'open', label: t('places.menu.open'), onSelect: () => open(path) },
      { id: 'open-tab', label: t('places.menu.openInTab'), icon: <ExternalLink aria-hidden />, shortcut: 'Mod+Enter', onSelect: () => window.open(path, '_blank', 'noopener') },
      { type: 'separator', id: 'separator-open' },
    );
  }
  entries.push(
    { id: 'move-shortcut', label: t('shortcuts.move'), icon: <FolderInput aria-hidden />, onSelect: () => openNodeDialog({ type: 'move-shortcut', shortcut, name }) },
    shortcut.node_id === null
      ? { id: 'unstar', label: t('places.star.unstar'), icon: <StarOff aria-hidden />, onSelect: () => remove.mutate() }
      : { id: 'remove-shortcut', label: t('shortcuts.remove'), icon: <Trash2 aria-hidden />, onSelect: () => remove.mutate() },
  );
  return entries;
}

// A shortcut's menus are made only while one is open, as a place's and a document's are: a row of a long list makes
// nothing for them, and its acts keep their work only for the row that is asked.
function ShortcutButtonEntries({ shortcut }: { shortcut: Shortcut }) {
  return <DropdownEntries entries={useShortcutEntries(shortcut)} />;
}

function ShortcutContextEntries({ shortcut }: { shortcut: Shortcut }) {
  return <ContextEntries entries={useShortcutEntries(shortcut)} />;
}

/** A shortcut's "⋯": the same list as its right-click menu. */
export function ShortcutMenuButton({ shortcut, skipTab = false }: { shortcut: Shortcut; skipTab?: boolean }) {
  const { t } = useTranslation();
  const name = shortcutName(t, shortcut);
  return (
    <Menu
      label={t('places.menu.of', { name })}
      align="end"
      content={<ShortcutButtonEntries shortcut={shortcut} />}
      trigger={<IconButton size="sm" label={t('places.menu.more', { name })} icon={<Ellipsis aria-hidden />} {...(skipTab ? { tabIndex: -1 } : {})} />}
    />
  );
}

/** Gives whatever it is put round the right-click menu of a shortcut. */
export function ShortcutContextMenu({ shortcut, children }: { shortcut: Shortcut; children: ReactElement }) {
  const { t } = useTranslation();
  return (
    <ContextMenu label={t('places.menu.of', { name: shortcutName(t, shortcut) })} content={<ShortcutContextEntries shortcut={shortcut} />}>
      {children}
    </ContextMenu>
  );
}

/** A chat's acts in its "⋯" with its shortcut acts after them, made only while the menu is open. */
export function ChatButtonEntries({ entries, target }: { entries: readonly MenuEntry[]; target: ShortcutTarget }) {
  return <DropdownEntries entries={[...entries, { type: 'separator', id: 'separator-shortcuts' }, ...useShortcutActs(target)]} />;
}

/** A chat's acts in its right-click menu with its shortcut acts after them, made only while the menu is open. */
export function ChatContextEntries({ entries, target }: { entries: readonly MenuEntry[]; target: ShortcutTarget }) {
  return <ContextEntries entries={[...entries, { type: 'separator', id: 'separator-shortcuts' }, ...useShortcutActs(target)]} />;
}

/**
 * A shortcut as a row of the tree or of a list: its icon with the arrow, its target's name, and where that lies,
 * leading to the target. One that does not open says why in place of a name, and leads nowhere; it is dragged onto
 * another place, or onto Starred, to be moved there.
 */
export function ShortcutLink({ shortcut, className, arrow = true, named = false, children }: { shortcut: Shortcut; className: string; arrow?: boolean; named?: boolean; children?: ReactNode }) {
  const { t } = useTranslation();
  const open = useOpenPath();
  const path = shortcutPath(shortcut);
  // A place or a thread the reader's tree holds is named as the tree names it now: a rename shows at once.
  const kind = shortcut.target.kind;
  const live = useTreeNode(shortcut.state === 'open' && (kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread') ? shortcut.target.id : null);
  const name = live?.name ?? shortcutName(t, shortcut);
  const carried: Carried = { id: shortcut.id, name, from: shortcut.node_id };
  const body = (
    <>
      <ShortcutIcon shortcut={shortcut} arrow={arrow} />
      <span className={cx('truncate', path === null && 'text-text-subtle italic')}>{name}</span>
      <span className="sr-only">{t('shortcuts.of', { kind: kindLabel(t, shortcut) })}</span>
      {children}
    </>
  );
  const drag = {
    draggable: true,
    onDragStart: (event: DragEvent) => {
      event.dataTransfer.setData(DRAGGED_SHORTCUT, JSON.stringify(carried));
      event.dataTransfer.effectAllowed = 'move';
    },
  };
  if (path === null) {
    return (
      <span className={className} title={name} data-shortcut={shortcut.id} data-row-name-place={named || undefined} {...drag}>
        {body}
      </span>
    );
  }
  return (
    <RoutedPathLink
      href={path}
      tabIndex={-1}
      className={className}
      data-shortcut={shortcut.id}
      data-row-name-place={named || undefined}
      {...drag}
      onClick={(event) => {
        // With a modifier or another button the browser's own way with a link is wanted: a new tab.
        if (!isPlainClick(event)) return;
        event.preventDefault();
        open(path);
      }}
    >
      {body}
    </RoutedPathLink>
  );
}

/** Stars or unstars a document or a chat: its shortcut in the person's own list. */
function useStarTarget(target: ShortcutTarget, starred: boolean): () => void {
  const { t } = useTranslation();
  const describe = useErrorText();
  const queryClient = useQueryClient();
  const change = useMutation({
    mutationFn: () => (starred ? unstarItem({ kind: target.kind, id: target.id }) : starItem({ kind: target.kind, id: target.id })),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: favoritesKey }),
    onError: (error) =>
      toast({
        tone: 'danger',
        title: starred ? t('places.star.notUnstarred', { name: target.name }) : t('places.star.notStarred', { name: target.name }),
        description: describe(error).message,
      }),
  });
  return () => change.mutate();
}

/** The entry that keeps a shortcut to a thing in a place the person chooses. */
export function addShortcutEntry(target: ShortcutTarget, t: TFunction): MenuEntry {
  return { id: 'add-shortcut', label: t('shortcuts.add'), icon: <CornerUpRight aria-hidden />, onSelect: () => openNodeDialog({ type: 'add-shortcut', target }) };
}

/** The entries a document's or a chat's menu has of shortcuts: star it, and keep a shortcut to it in a place. */
export function useShortcutActs(target: ShortcutTarget): MenuEntry[] {
  const { t } = useTranslation();
  const starred = useIsStarred(target.id);
  const star = useStarTarget(target, starred);
  return [
    starred
      ? { id: 'unstar', label: t('places.star.unstar'), icon: <StarOff aria-hidden />, onSelect: star }
      : { id: 'star', label: t('places.star.star'), icon: <Star aria-hidden />, onSelect: star },
    addShortcutEntry(target, t),
  ];
}

/** One shortcut as a row of a place's list: the same link, menu and drag as its row in the tree. */
function ShortcutListRow({ shortcut }: { shortcut: Shortcut }) {
  const { t } = useTranslation();
  const where = shortcut.item?.path.at(-1);
  return (
    <ShortcutContextMenu shortcut={shortcut}>
      <li className="group flex items-center gap-3 px-3 py-2" data-shortcut-row={shortcut.id}>
        <ShortcutLink shortcut={shortcut} className={cx('flex min-w-0 flex-1 items-center gap-2 text-base text-text outline-none', shortcutPath(shortcut) !== null && 'hover:underline')}>
          {where !== undefined && <span className="truncate text-sm text-text-muted">{where.redacted || where.name === null ? t('ui.nameUnknown') : where.name}</span>}
        </ShortcutLink>
        <ShortcutMenuButton shortcut={shortcut} />
      </li>
    </ShortcutContextMenu>
  );
}

/**
 * The shortcuts kept in a place, under its list of documents. They are read only where the place's row in the tree
 * says it keeps some, so a place that keeps none asks for nothing more when its list opens.
 */
export function PlaceShortcuts({ nodeId }: { nodeId: string }) {
  const { t } = useTranslation();
  const kept = useTreeNode(nodeId)?.shortcuts ?? 0;
  const list = useInfiniteQuery({ ...shortcutsOfPlaceQuery(nodeId), enabled: kept > 0 });
  const shortcuts = list.data?.pages.flatMap((page) => page.items) ?? [];
  // Somebody kept, moved or removed one here: the place's row says so on its tree's feed, and the list is read again.
  const counted = useRef(kept);
  const { refetch } = list;
  useEffect(() => {
    if (counted.current === kept) return;
    counted.current = kept;
    if (kept > 0) void refetch();
  }, [kept, refetch]);
  if (kept === 0 && shortcuts.length === 0) return null;
  return (
    <section aria-label={t('shortcuts.title')} className="flex flex-col gap-2">
      <h3 className="text-sm font-medium text-text-muted">{t('shortcuts.title')}</h3>
      {list.isPending ? (
        <SkeletonRows read={list} rows={Math.min(kept, 3)} />
      ) : list.isError && shortcuts.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : (
        <ul className="divide-y divide-border rounded-lg border border-border bg-surface">
          {shortcuts.map((shortcut) => (
            <ShortcutListRow key={shortcut.id} shortcut={shortcut} />
          ))}
        </ul>
      )}
      <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : null} onMore={() => void list.fetchNextPage()} />
    </section>
  );
}
