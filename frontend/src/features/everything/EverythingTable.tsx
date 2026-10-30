/** A lazily read tree grid: safe paths, aligned optional columns, a sticky header and the tree's keyboard. */
import { ContainedThreadStateIcon, ThreadStateIcon } from '@/ui/ThreadState';
import { useInfiniteQuery } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, Bot, Box, Boxes, ChevronDown, ChevronRight, Copy, Filter, FolderTree, GraduationCap, HardDrive, LayoutTemplate, LockKeyhole, MessageSquare, MoreHorizontal, Plug, PlugZap, Server } from 'lucide-react';
import { createContext, Fragment, type CSSProperties, type KeyboardEvent, memo, type ReactNode, useCallback, useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { everythingQuery, type EverythingFilter } from '@/api/everything';
import { openAccess } from '@/features/access/AccessDialog';
import { COLUMN_SORT, type Column } from '@/features/everything/columns';
import { groupedRecents, recentAt, recentGroup } from '@/features/navigation/recent';
import { showInTree } from '@/features/navigation/showInTree';
import { useAppearance } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { useListingRetired, useRecentUpdates } from '@/features/navigation/useRecentUpdates';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { EverythingRow, UserRef } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { ByteSize } from '@/ui/ByteSize';
import { ContextMenu } from '@/ui/ContextMenu';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { NodeIcon } from '@/ui/NodeIcon';
import { RelativeTime } from '@/ui/RelativeTime';
import { nearestPlace, RowPath, RowPlace } from '@/ui/RowDetails';
import { isWorkState, StateIcon } from '@/ui/StateChip';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';
import { Checkbox } from '@/ui/Checkbox';
import { copyText } from '@/ui/clipboard';
import './everything-rows.css';

/** Desktop cells share the row-height token; narrow rows grow to show their labelled details. */
const CELL = 'min-w-0 px-3 py-2 text-start align-middle text-sm wrap-anywhere text-text-muted sm:h-(--table-row-height) sm:border-b sm:border-border sm:py-1 sm:wrap-normal sm:whitespace-nowrap';
const ICON_CLASS = 'size-4 shrink-0 text-text-subtle';
// Keep the existing minimum scrolling width; actual Name sizing follows its content.
const MIN_NAME_SHARE = 0.23;
// Five rem holds Where and its sort arrow; extra columns widen the table, not their text.
const PLACE_MIN_REM = 5;
const ACTION_REM = 2.5;
const CHOICE_REM = 2;
const COLUMN_REM: Record<Exclude<Column, 'name' | 'where'>, number> = {
  admin_access: 5, kind: 7, owner: 10, created: 7.5, created_by: 10, last_activity: 7.5,
  state: 5, size: 7, my_access: 8, tags: 12, opened: 7.5, activity_by: 7.5,
};
const RESOURCE_ICONS = new Map<string, ReactNode>(Object.entries({
  environment: <Boxes className={ICON_CLASS} aria-hidden />,
  provider: <PlugZap className={ICON_CLASS} aria-hidden />,
  agent: <Bot className={ICON_CLASS} aria-hidden />,
  mcp_server: <Plug className={ICON_CLASS} aria-hidden />,
  host: <Server className={ICON_CLASS} aria-hidden />,
  machine: <Server className={ICON_CLASS} aria-hidden />,
  mount: <HardDrive className={ICON_CLASS} aria-hidden />,
  template: <LayoutTemplate className={ICON_CLASS} aria-hidden />,
  skill: <GraduationCap className={ICON_CLASS} aria-hidden />,
  chat: <MessageSquare className={ICON_CLASS} aria-hidden />,
  group: <FolderTree className={ICON_CLASS} aria-hidden />,
}));

interface TreeState {
  extraColumns: readonly Column[];
  selection?: EverythingSelection | undefined;
  rememberRow: (key: string, row: EverythingRow | null) => void;
  reader: string;
  columns: readonly Column[];
  filter: EverythingFilter;
  all: boolean;
  closed: ReadonlySet<string>;
  opened: ReadonlySet<string>;
  toggle: (key: string, open: boolean) => void;
  active: string | null;
  focus: (key: string) => void;
  onOwner?: ((owner: UserRef) => void) | undefined;
}
const Tree = createContext<TreeState | null>(null);
function useTree(): TreeState {
  const tree = useContext(Tree);
  if (tree === null) throw new Error('The table needs its tree state.');
  return tree;
}
const keyOf = (row: EverythingRow) => `${row.kind}:${row.id}`;

export interface EverythingSelection {
  selected: ReadonlySet<string>;
  toggle: (row: EverythingRow, selected: boolean) => void;
  all: (rows: readonly EverythingRow[]) => void;
  clear: () => void;
}

/** Listing selection serves the same Share act as its rows; the dialog rechecks every page. */
export function shareableRow(row: EverythingRow): boolean {
  return row.my_access !== 'path_only' && row.can_share && row.kind !== 'group' && row.kind !== 'machine' && row.kind !== 'chat';
}

/** Flat rows with a State column keep identity and activity in separate first-line slots. */
export function KindIcon({ row, activity = true }: { row: { id: string; kind: string; path: EverythingRow['path']; thread_display?: EverythingRow['thread_display'] }; activity?: boolean }) {
  const kind = row.kind;
  if (kind === 'thread' && activity) return <ThreadStateIcon display={row.thread_display} />;
  if (kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread' || kind === 'document') {
    const parent = row.path.filter((step) => step.id !== row.id).at(-1);
    return <NodeIcon kind={kind === 'project' && parent?.kind === 'project' ? 'folder' : kind} />;
  }
  return RESOURCE_ICONS.get(kind) ?? <Box className={ICON_CLASS} aria-hidden />;
}

function Cell({ column, row, onOwner }: { column: Column; row: EverythingRow; onOwner: TreeState['onOwner'] }): ReactNode {
  const { t } = useTranslation();
  if (row.kind === 'group' || (row.my_access === 'path_only' && column !== 'kind' && column !== 'my_access')) return null;
  switch (column) {
    case 'name': return null;
    case 'admin_access': return <AsAdminMark of={adminAccessOf(row)?.place} responsive />;
    case 'kind': return t(`everything.kinds.${row.kind}`, { defaultValue: t('everything.unknownKind') });
    case 'owner': return row.owner === null ? '—' : <span className="inline-flex items-center gap-1"><UserChip user={row.owner} thing={{ kind: row.kind, id: row.id }} />{onOwner !== undefined && <IconButton size="sm" label={t('everything.onlyOwner', { name: row.owner.display_name })} icon={<Filter aria-hidden />} onClick={() => row.owner !== null && onOwner(row.owner)} />}</span>;
    case 'created_by': return row.created_by === null ? '—' : <UserChip user={row.created_by} thing={{ kind: row.kind, id: row.id }} />;
    case 'activity_by': return row.activity_by === undefined || row.activity_by === null ? '—' : <UserChip className="max-w-full" user={row.activity_by} thing={{ kind: row.kind, id: row.id }} />;
    case 'where': return row.place_label === null ? '—' : <RowPath path={row.place_label} />;
    case 'opened': return row.last_opened === undefined || row.last_opened === null ? '—' : <RelativeTime compact at={row.last_opened} />;
    case 'created': return row.created_at === null ? '—' : <RelativeTime at={row.created_at} />;
    case 'last_activity': return row.last_activity === null ? '—' : <RelativeTime at={row.last_activity} />;
    case 'state': return row.kind === 'thread'
      ? row.thread_display === null || row.thread_display === undefined ? '—' : t(`state.${row.thread_display.state}`)
      : row.state === null ? '—' : t(`everything.states.${row.state}`, { defaultValue: t('everything.unknown') });
    case 'my_access': return t(`everything.access.${row.my_access}`);
    case 'tags': return row.tags.length === 0 ? '—' : <span className="flex flex-wrap gap-1">{row.tags.map((tag) => <span key={tag.id} className="rounded-sm bg-bg-muted px-1">{tag.name}</span>)}</span>;
    case 'size': return <span className="flex flex-col gap-1">{row.size_bytes === null ? '—' : <ByteSize bytes={row.size_bytes} />}{row.takes !== null && <span className="text-xs">{[
      row.takes.cpus === null ? null : t('everything.cpus', { count: row.takes.cpus }),
      row.takes.memory_bytes === null ? null : <span key="memory">{t('everything.memory')} <ByteSize bytes={row.takes.memory_bytes} /></span>,
      row.takes.storage_bytes === null ? null : <span key="storage">{t('everything.storage')} <ByteSize bytes={row.takes.storage_bytes} /></span>,
    ].filter((one) => one !== null).map((one, index) => <Fragment key={index}>{index === 0 ? null : ' · '}{one}</Fragment>)}</span>}</span>;
  }
}

function RecordedTime({ column, row }: { column: 'opened' | 'created' | 'last_activity'; row: EverythingRow }) {
  const at = (column === 'opened' ? row.last_opened : column === 'created' ? row.created_at : row.last_activity) ?? null;
  return at === null ? '—' : <RelativeTime compact at={at} />;
}

const Row = memo(function Row({ row, parentKey, depth, first, stale = false }: { row: EverythingRow; parentKey: string | null; depth: number; first: boolean; stale?: boolean }) {
  const { t } = useTranslation();
  const tree = useTree();
  const key = keyOf(row);
  const recent = tree.filter.recent;
  const flat = recent !== undefined;
  const timeColumn = recent === 'opened' ? 'opened' : 'last_activity';
  const place = nearestPlace(row);
  const actor = row.activity_by ?? null;
  const state = row.kind === 'thread'
    ? row.my_access === 'path_only' || row.thread_display === null || row.thread_display === undefined ? null : <ThreadStateIcon display={row.thread_display} />
    : row.state !== null && isWorkState(row.state) ? <StateIcon state={row.state} label={t(`everything.states.${row.state}`, { defaultValue: t('everything.unknown') })} /> : null;
  const expandable = row.has_children && tree.filter.q === '' && tree.filter.recent === undefined;
  const open = expandable && (tree.all ? !tree.closed.has(key) : tree.opened.has(key));
  const name = row.group === null ? (row.name ?? t('everything.privatePlace')) : t(`everything.groups.${row.group}`);
  const label = row.place_label === null ? name : t('everything.placedName', { name, place: row.place_label });
  const pathOnly = row.my_access === 'path_only' && row.kind !== 'group';
  const rowRef = useRef<HTMLTableRowElement>(null);
  const actions: MenuEntry[] = [];
  if (row.href !== null) actions.push({ id: 'open', label: t('everything.open'), href: row.href, onSelect: () => undefined });
  if (row.href !== null) actions.push({ id: 'copy-link', label: t('places.menu.copyLink'), icon: <Copy aria-hidden />, onSelect: () => { void copyText(new URL(row.href ?? '', location.origin).href); } });
  const shareKind = row.kind;
  if (row.can_share && shareKind !== 'group' && shareKind !== 'machine' && shareKind !== 'chat') actions.push({ id: 'share', label: t('everything.share'), onSelect: () => openAccess({ kind: shareKind, id: row.id, name }) });
  if (row.settings_href !== null) actions.push({ id: 'settings', label: t('everything.settings'), href: row.settings_href, onSelect: () => undefined });
  if (row.href !== null && row.path.length > 0 && ['org', 'project', 'topic', 'thread', 'document', 'chat'].includes(row.kind)) actions.push({ id: 'show-in-tree', label: t('navigation.showInTree'), onSelect: () => showInTree(row) });
  const press = (event: KeyboardEvent<HTMLTableRowElement>) => {
    if (event.target !== event.currentTarget) return;
    const rows = Array.from(event.currentTarget.closest('table')?.querySelectorAll<HTMLTableRowElement>('[data-everything-row]') ?? []);
    const at = rows.indexOf(event.currentTarget);
    let target: HTMLTableRowElement | undefined;
    switch (event.key) {
      case 'ArrowDown': target = rows[at + 1]; break;
      case 'ArrowUp': target = rows[at - 1]; break;
      case 'Home': target = rows[0]; break;
      case 'End': target = rows.at(-1); break;
      case 'ArrowRight':
        if (expandable && !open) tree.toggle(key, true);
        else if (open && rows[at + 1]?.dataset.parentKey === key) target = rows[at + 1];
        break;
      case 'ArrowLeft':
        if (open) tree.toggle(key, false);
        else target = rows.find((one) => one.dataset.everythingRow === parentKey);
        break;
      case 'Enter':
        if (row.href !== null) rowRef.current?.querySelector<HTMLAnchorElement>('a[data-open]')?.click();
        else if (expandable) tree.toggle(key, !open);
        break;
      case ' ':
        if (tree.selection !== undefined && shareableRow(row)) tree.selection.toggle(row, !tree.selection.selected.has(key));
        else return;
        break;
      default: return;
    }
    event.preventDefault();
    if (target !== undefined) { tree.focus(target.dataset.everythingRow ?? ''); target.focus(); }
  };
  return <>
    <ContextMenu label={t('everything.actions', { name })} entries={actions}><tr ref={(element) => { rowRef.current = element; tree.rememberRow(key, element === null ? null : row); }} role="row" data-everything-row={key} data-parent-key={parentKey ?? ''} aria-level={depth + 1} aria-expanded={expandable ? open : undefined}
      aria-selected={tree.selection === undefined ? undefined : tree.selection.selected.has(key)}
      tabIndex={tree.active === key || (tree.active === null && first) ? 0 : -1} onFocus={() => tree.focus(key)} onKeyDown={press}
      className={`focus-inset ${flat ? 'everything-flat-row' : 'grid grid-cols-2 border-b border-border py-2'} hover:bg-bg-muted sm:table-row sm:border-0 sm:py-0 ${stale ? 'opacity-60' : ''}`} data-selecting={tree.selection !== undefined || undefined}>
      {tree.selection !== undefined && <td role="gridcell" className={`${CELL} ${flat ? 'flat-choice' : 'col-start-1 row-start-1 z-(--layer-sticky) w-fit justify-self-start sm:w-auto'}`}>
        {shareableRow(row) && <Checkbox presentation="compact" label={<span className="sr-only">{t('everything.selectNamed', { name: label })}</span>} checked={tree.selection.selected.has(key)} onCheckedChange={(selected) => tree.selection?.toggle(row, selected)} />}
      </td>}
      <td role="gridcell" className={`${CELL} ${flat ? 'flat-name' : 'col-span-2 col-start-1 row-start-1 pr-[calc(var(--control-sm)+var(--spacing-3)+var(--spacing-2))] sm:pr-3'}`} style={flat ? undefined : { paddingInlineStart: `calc(var(--spacing-4) * ${depth} + var(--spacing-2))` }}>
        <span className={`flex min-w-0 items-center gap-2 ${!flat && tree.selection !== undefined ? 'pl-8 sm:pl-0' : ''}`}>
          {!flat && (expandable ? <IconButton size="sm" label={t(open ? 'everything.collapse' : 'everything.expand', { name: label })} icon={open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />} aria-expanded={open} onClick={() => tree.toggle(key, !open)} /> : <span className="size-(--control-sm) shrink-0" aria-hidden />)}
          <KindIcon row={row} activity={!flat || !tree.columns.includes('state')} />
          <span className={`flex min-w-0 flex-1 flex-col gap-1 ${flat ? '' : 'wrap-anywhere sm:min-w-[12ch] sm:wrap-normal'}`}>
            {row.href === null ? <span title={name} className={`font-medium text-text ${flat ? 'truncate' : ''}`}>{name}</span> : <a data-open title={name} aria-label={label} href={row.href} className={`rounded-md font-medium text-text underline-offset-2 hover:underline ${flat ? 'truncate' : ''}`}>{name}</a>}
            {row.place_label !== null && !tree.columns.includes('where') && <RowPlace value={place} />}
          </span>
          {!pathOnly && (row.kind === 'org' || row.kind === 'project' || row.kind === 'topic') && <ContainedThreadStateIcon id={row.id} />}
          {!tree.columns.includes('admin_access') && <AsAdminMark of={adminAccessOf(row)?.place} responsive />}
          {pathOnly && !tree.columns.includes('my_access') && <span className="flex shrink-0 items-center gap-1 text-xs text-text-subtle"><LockKeyhole className="size-4" aria-hidden />{t('everything.access.path_only')}</span>}
        </span>
      </td>
      {flat && recent === 'opened' && actor !== null && <td role="gridcell" className={`${CELL} flat-by flat-phone-by`}><span className="text-xs text-text-subtle">{t('everything.columns.activity_by')}</span><UserChip className="max-w-full" user={actor} {...(row.kind === 'group' || pathOnly ? {} : { thing: { kind: row.kind, id: row.id } })} /></td>}
      {tree.columns.filter((column) => column !== 'name').map((column) => <td role="gridcell" key={column} data-column={column} className={`${CELL} ${flat ? column === 'where' ? 'flat-place' : column === timeColumn ? 'flat-time' : column === 'activity_by' ? 'flat-by' : column === 'state' ? 'flat-state' : tree.extraColumns.includes(column) ? 'flat-extra' : 'flat-secondary' : row.kind === 'group' || pathOnly && column !== 'kind' && column !== 'my_access' ? 'hidden sm:table-cell' : 'block sm:table-cell'}`} data-empty={(column === 'activity_by' && actor === null) || (column === 'state' && state === null) || undefined}>
        {row.kind !== 'group' && (!pathOnly || column === 'kind' || column === 'my_access') && <>
          <span className={`${flat ? 'flat-field-label' : 'mb-1 block'} text-xs text-text-subtle sm:hidden`}>{t(`everything.columns.${column}`)}</span>
          {flat && column === 'where' ? <><span data-mobile-place><RowPlace value={place} /></span><span data-desktop-place className="min-w-0"><Cell column={column} row={row} onOwner={tree.onOwner} /></span></>
            : flat && (column === 'opened' || column === 'last_activity' || column === 'created') ? <RecordedTime column={column} row={row} />
              : flat && column === 'state' ? state ?? <span className="hidden sm:inline">—</span>
                : <Cell column={column} row={row} onOwner={tree.onOwner} />}
        </>}
      </td>)}
      <td role="gridcell" className={`${CELL} ${flat ? 'flat-actions' : 'col-start-2 row-start-1 justify-self-end sm:table-cell'}`}>{actions.length > 0 && !pathOnly && <Menu label={t('everything.actions', { name })} entries={actions} trigger={<IconButton label={t('everything.actions', { name })} size="sm" icon={<MoreHorizontal aria-hidden />} />} />}</td>
    </tr></ContextMenu>
    {open && <Level parent={row.id} parentKey={key} depth={depth + 1} />}
  </>;
});

function Level({ parent, parentKey, depth }: { parent: string | null; parentKey: string | null; depth: number }) {
  const { t } = useTranslation();
  const tree = useTree();
  const now = useNow();
  const timeZone = useAppearance((state) => state.timeZone);
  const shown = useViewShown();
  const options = everythingQuery(tree.reader, parent, tree.filter);
  const level = useInfiniteQuery({ ...options, enabled: shown });
  const retired = useListingRetired(options.queryKey, level.error);
  const clearSelection = tree.selection?.clear;
  useEffect(() => { if (retired) clearSelection?.(); }, [retired, clearSelection]);
  useRecentUpdates(options.queryKey, shown && (tree.filter.recent !== undefined || tree.filter.sort === 'opened'), tree.filter.recent === 'opened' || tree.filter.sort === 'opened');
  const received = level.data?.pages.flatMap((page) => page.items) ?? [];
  const rows = tree.filter.recent === undefined ? received : groupedRecents(received, tree.filter.recent, now, timeZone);
  const filtering = tree.filter.kinds.length > 0 || tree.filter.owner !== null || tree.filter.createdFrom !== '' || tree.filter.createdTo !== '' || (tree.filter.activeFrom ?? '') !== '' || tree.filter.participating === true || tree.filter.adminAccess !== undefined;
  const { hasNextPage, isFetching, isError, fetchNextPage } = level;
  // Expand all includes the pages of each branch; an ordinary expansion keeps its explicit Show more.
  useEffect(() => {
    if (shown && tree.all && hasNextPage && !isFetching && !isError) void fetchNextPage();
  }, [shown, tree.all, hasNextPage, isFetching, isError, fetchNextPage]);
  const span = tree.columns.length + 1 + (tree.selection === undefined ? 0 : 1);
  if (level.isPending) return <tr role="row"><td role="gridcell" colSpan={span} className={CELL}><SkeletonRows rows={3} /></td></tr>;
  const failure = level.isError ? <tr role="row"><td role="gridcell" colSpan={span} className={CELL}>{level.error instanceof ApiError && level.error.status === 403 ? <><p>{t('everything.refused')}</p><Button size="sm" asChild><a href="/everything">{t('everything.openMine')}</a></Button></> : <><ErrorAlert error={level.error} /><Button size="sm" variant="ghost" onClick={() => void level.refetch()}>{t('everything.retry')}</Button></>}</td></tr> : null;
  if (level.isError && (rows.length === 0 || retired)) return failure;
  if (rows.length === 0) return <tr role="row"><td role="gridcell" colSpan={span} className={CELL}>{t(tree.filter.q !== '' ? 'everything.noMatches' : filtering ? 'everything.noResults' : 'everything.empty')}</td></tr>;
  return <>
    {failure}
    {rows.map((row, index) => {
      const recent = tree.filter.recent;
      const group = recent === undefined ? null : recentGroup(recentAt(row, recent), now, timeZone);
      const previous = rows[index - 1];
      const heading = group !== null && (previous === undefined || recent === undefined || recentGroup(recentAt(previous, recent), now, timeZone) !== group);
      return <Fragment key={keyOf(row)}>{heading && <tr role="row"><th role="rowheader" colSpan={span} className="bg-bg-subtle px-3 pt-3 pb-2 text-left text-sm font-medium text-text-muted">{t(`navigation.recentGroups.${group}`)}</th></tr>}
        <Row row={row} parentKey={parentKey} depth={depth} first={depth === 0 && index === 0} stale={level.isError} /></Fragment>;
    })}
    {depth === 0 && filtering && rows.every((row) => row.kind === 'group' && !row.has_children) && <tr role="row"><td role="gridcell" colSpan={span} className={CELL}>{t('everything.noResults')}</td></tr>}
    {level.hasNextPage && <tr role="row"><td role="gridcell" colSpan={span} className={CELL}><Button size="sm" variant="ghost" pending={level.isFetchingNextPage} onClick={() => void level.fetchNextPage()}>{t('everything.more')}</Button></td></tr>}
  </>;
}

export interface EverythingTableProps {
  extraColumns?: readonly Column[];
  selection?: EverythingSelection | undefined;
  reader: string;
  filter: EverythingFilter;
  columns: readonly Column[];
  parent?: string | null;
  expansion: 'all' | 'none' | 'manual';
  onSort: (column: Column) => void;
  onOwner?: ((owner: UserRef) => void) | undefined;
}

/** The table owns focus and expansion; a new filter starts at its first row. */
export function EverythingTable({ reader, filter, columns, extraColumns = [], parent = null, expansion, onSort, onOwner, selection: selecting }: EverythingTableProps) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
  const displayed = useRef(new Map<string, EverythingRow>());
  const rememberRow = useCallback((key: string, row: EverythingRow | null) => { if (row === null) displayed.current.delete(key); else displayed.current.set(key, row); }, []);
  const selection = JSON.stringify(filter);
  const [focused, setFocused] = useState<{ selection: string; key: string } | null>(null);
  const active = focused?.selection === selection ? focused.key : null;
  const focus = (key: string) => setFocused({ selection, key });
  const toggle = (key: string, open: boolean) => {
    if (expansion === 'all') setClosed((before) => { const next = new Set(before); if (open) next.delete(key); else next.add(key); return next; });
    else setOpened((before) => { const next = new Set(before); if (open) next.add(key); else next.delete(key); return next; });
  };
  const recent = filter.recent !== undefined;
  const fixedRem = columns.reduce((sum, column) => sum + (column === 'name' || column === 'where' ? 0 : COLUMN_REM[column]), ACTION_REM + (selecting === undefined ? 0 : CHOICE_REM));
  const tracks = [
    ...(selecting === undefined ? [] : [`${CHOICE_REM}rem`]),
    ...columns.map((column) => column === 'name' ? 'minmax(0, max-content)' : column === 'where' ? `minmax(${PLACE_MIN_REM}rem, 1fr)` : `${COLUMN_REM[column]}rem`),
    `${ACTION_REM}rem`,
  ].join(' ');
  const size = { '--recent-min-width': `${(fixedRem + PLACE_MIN_REM) / (1 - MIN_NAME_SHARE)}rem`, '--recent-columns': tracks } as CSSProperties;
  return <Tree.Provider value={{ reader, filter, columns, extraColumns, all: expansion === 'all', opened, closed, toggle, active, focus, onOwner, selection: selecting, rememberRow }}>
    <table role="treegrid" aria-label={t('everything.tree')} className={`block w-full border-separate border-spacing-0 sm:table ${recent ? 'everything-recent-table' : ''}`} style={recent ? size : undefined}>
      {selecting !== undefined && <caption className="block px-3 py-2 text-left sm:hidden"><Button size="sm" variant="ghost" onClick={() => selecting.all([...displayed.current.values()].filter(shareableRow))}>{t('everything.selectAll')}</Button></caption>}
      <thead className="sticky top-0 z-(--layer-sticky) hidden bg-surface sm:table-header-group"><tr role="row">
        {selecting !== undefined && <th role="columnheader" scope="col" className={CELL}><Button size="sm" variant="ghost" onClick={() => selecting.all([...displayed.current.values()].filter(shareableRow))}>{t('navigation.recentAll')}</Button></th>}
        {columns.map((column) => <th role="columnheader" key={column} scope="col" className={CELL} aria-sort={column !== 'admin_access' && filter.sort === COLUMN_SORT[column] ? (filter.direction === 'asc' ? 'ascending' : 'descending') : 'none'}>
          {column === 'admin_access' ? <span>{t('adminReach.mode.name')}</span> : <button type="button" title={column === 'size' ? t('everything.sizeHint') : undefined} onClick={() => onSort(column)} className="flex items-center gap-2 rounded-md font-medium text-text whitespace-nowrap">
            {t(filter.recent === 'active' && column === 'last_activity' ? 'navigation.recentOrder.active' : `everything.columns.${column}`)}{filter.sort === COLUMN_SORT[column] && (filter.direction === 'asc' ? <ArrowUp className="size-4" aria-hidden /> : <ArrowDown className="size-4" aria-hidden />)}
          </button>}
        </th>)}
        <th role="columnheader" scope="col" className={CELL}><span className="sr-only">{t('everything.rowActions')}</span></th>
      </tr></thead>
      <tbody className="block sm:table-row-group"><Level parent={parent} parentKey={null} depth={0} /></tbody>
    </table>
  </Tree.Provider>;
}
