/** The tree of a workspace: lazy, drawn by the rows in view, and kept equal to the disk.
 *
 * Directories are read when they are opened and watched while they stay open. The rows are one flat
 * list laid out from what is loaded, and only rows near the viewport are drawn. Directory read
 * limits are shown independently of virtualization. The keyboard works it as a tree is worked
 * anywhere (arrows, Home and End, a letter, Enter to open, F2 to rename, Delete to delete), and every
 * row has the menu of a file manager under the right mouse button, the same as its "⋯" and `Shift+F10`.
 */
import { useVirtualizer } from '@tanstack/react-virtual';
import { ChevronRight, File as FileIcon, FileSymlink, Folder, FolderOpen, MoreHorizontal } from 'lucide-react';
import { type KeyboardEvent, type MouseEvent, memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { formatDateTime } from '@/lib/time';

import type { Door } from '@/api/workspace';
import type { DirEntry } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { isComposing } from '@/lib/shortcuts';
import { type DirectoryState, type DirectoryWindow, directoryKey } from '@/realtime/stores/environment';
import { DirectoryPager } from './DirectoryPager';
import { readWorkspaceDrag, workspaceDrag } from './drag';
import { toast } from '@/ui/Toast';
import { ContextEntries, ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';
import { Button } from '@/ui/Button';
import { environmentFileReference } from '@/features/people/references';
import { ORCH_REFERENCE } from '@/lib/drop';
import { useErrorText } from '@/ui/errorText';
import { FileChip } from './FileChip';
import { workspaceSource } from './source';

/** What a row of the tree is. */
export type TreeRowKind = 'entry' | 'loading' | 'empty' | 'more' | 'failed' | 'changed' | 'page' | 'capacity';

export interface FileRow {
  /** Unique in the tree: the path of an entry, or the path of its directory with the kind of the note. */
  id: string;
  kind: TreeRowKind;
  entry: DirEntry | null;
  depth: number;
  /** The directory it lies in. */
  parent: string;
  expandable: boolean;
  expanded: boolean;
  /** Where it stands among the rows under the same directory, from one, and how many there are. */
  position: number;
  setSize: number;
  /** The row is from a cached view of its directory, where only what Orch keeps a copy of can be opened. */
  cachedView: boolean;
  error?: unknown;
  reason?: string | undefined;
  retryable?: boolean;
  window?: DirectoryWindow;
}

export function focusableFileRow(row: FileRow): boolean {
  return row.kind === 'entry' || row.kind === 'page' || row.kind === 'capacity' || (row.kind === 'failed' || row.kind === 'changed') && row.retryable !== false;
}

/** The rows of a tree, laid out from the directories that are loaded and the ones that are open. */
export function layoutRows(root: string, expanded: ReadonlySet<string>, dirs: ReadonlyMap<string, DirectoryState>, environmentId: string): FileRow[] {
  const rows: FileRow[] = [];
  const visit = (directory: string, depth: number) => {
    const state = dirs.get(directoryKey(environmentId, directory));
    const window = state !== undefined && 'page' in state ? state as DirectoryWindow : undefined;
    if (window?.capacity !== undefined && window.capacity !== null && window.entries.length === 0) {
      rows.push({ id: `${directory}\u0000capacity`, kind: 'capacity', entry: null, depth, parent: directory, expandable: false, expanded: false, position: 1, setSize: 1, cachedView: false });
      return;
    }
    if (state === undefined || (state.status === 'loading' && state.entries.length === 0)) {
      rows.push({ id: `${directory}\u0000loading`, kind: 'loading', entry: null, depth, parent: directory, expandable: false, expanded: false, position: 1, setSize: 1, cachedView: false });
      return;
    }
    if (state.status === 'failed' && state.entries.length === 0) {
      rows.push({ id: `${directory}\u0000failed`, kind: 'failed', entry: null, depth, parent: directory, expandable: false, expanded: false, position: 1, setSize: 1, cachedView: false,
        error: state.error, reason: state.watchError?.detail, retryable: state.watchError?.retryable ?? state.watch !== 'refused' });
      return;
    }
    if (state.invalidated === true || state.status === 'failed') {
      const kind = state.status === 'failed' ? 'failed' : 'changed';
      rows.push({ id: `${directory}\u0000${kind}`, kind, entry: null, depth, parent: directory, expandable: false, expanded: false,
        position: 1, setSize: 1, cachedView: false, error: state.error, reason: state.watchError?.detail, retryable: state.watchError?.retryable ?? state.watch !== 'refused' });
      if (state.entries.length === 0) return;
    }
    if (state.entries.length === 0 && !state.truncated && (window?.page?.ordinal ?? 1) === 1) {
      rows.push({ id: `${directory}\u0000empty`, kind: 'empty', entry: null, depth, parent: directory, expandable: false, expanded: false, position: 1, setSize: 1, cachedView: false });
      return;
    }
    const count = state.entries.length;
    const setSize = window !== undefined && (window.truncated || (window.page?.ordinal ?? 1) > 1) ? -1 : count;
    const cachedView = state.source === 'cached';
    state.entries.forEach((entry, index) => {
      const isDir = entry.type === 'dir';
      const open = isDir && expanded.has(entry.path);
      rows.push({ id: entry.path, kind: 'entry', entry, depth, parent: directory, expandable: isDir, expanded: open, position: index + 1, setSize, cachedView });
      if (open) visit(entry.path, depth + 1);
    });
    if (window !== undefined) {
      if (window.truncated && !window.canNext && window.invalidated !== true) rows.push({ id: `${directory}\u0000more`, kind: 'more', entry: null, depth, parent: directory, expandable: false, expanded: false, position: count, setSize: count, cachedView: false });
      if (depth > 0 && (window.canNext || (window.page?.ordinal ?? 1) > 1 || window.restartRequired || window.capacity !== null)) {
        rows.push({ id: `${directory}\u0000page`, kind: 'page', entry: null, depth, parent: directory, expandable: false, expanded: false, position: count, setSize: count, cachedView: false, window });
      }
    } else if (state.truncated && state.invalidated !== true) rows.push({ id: `${directory}\u0000more`, kind: 'more', entry: null, depth, parent: directory, expandable: false, expanded: false, position: count, setSize: count, cachedView: false });
  };
  visit(root, 0);
  return rows;
}

/** The height of a row in pixels, from the token the density sets; read again when the density changes. */
export function useRowHeight(): number {
  const [height, setHeight] = useState(36);
  useLayoutEffect(() => {
    const read = () => {
      const style = getComputedStyle(document.documentElement);
      const rem = Number.parseFloat(style.fontSize) || 16;
      const value = style.getPropertyValue('--row-height').trim();
      const number = Number.parseFloat(value);
      if (Number.isFinite(number)) setHeight(value.endsWith('rem') ? number * rem : number);
    };
    read();
    const watch = new MutationObserver(read);
    watch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-density', 'style'] });
    return () => watch.disconnect();
  }, []);
  return height;
}

export interface FileTreeProps {
  /** Current visible rows, including live additions, for toolbar and batch operations. */
  onRowsChange?: (rows: readonly FileRow[]) => void;
  door: Door;
  directories: ReadonlyMap<string, DirectoryWindow>;
  onNavigate: (path: string, action: 'previous' | 'next' | 'restart') => void;
  /** The directory the tree is opened at: its children are the first rows. */
  root: string;
  /** What the tree is of, for a screen reader. */
  label: string;
  expanded: ReadonlySet<string>;
  onExpandedChange: (path: string, expanded: boolean) => void;
  /** The entries that are selected, and the one the keyboard is on. */
  selected: ReadonlySet<string>;
  focused: string | null;
  onFocused: (path: string | null) => void;
  /** A click on a row: the plain click, with Ctrl or Command, or with Shift. */
  onSelect: (entry: DirEntry, how: 'single' | 'toggle' | 'range', rows: readonly FileRow[]) => void;
  /** A file is opened: by a click, or Enter (`permanent` on a double click). */
  onOpen: (entry: DirEntry, permanent: boolean) => void;
  /** The menu of a row, as entries, made only when it is opened. */
  menuFor: (entry: DirEntry, selection: ReadonlySet<string>) => MenuEntry[];
  /** A key on a row that stands for an act of the menu: F2, Delete. */
  onKey: (key: 'rename' | 'delete' | 'copy' | 'cut' | 'paste', entry: DirEntry) => void;
  /** Rows were dropped on a directory. */
  onDropOn?: (target: DirEntry, paths: readonly string[]) => void;
  /** The folder an external upload is over; only that row carries its outline. */
  uploadOver?: string | null;
  /** The tree was asked for the rows again after a failure. */
  onRetry: (directory: string) => void;
  /** The menu of the empty part under the rows. */
  emptyEntries?: readonly MenuEntry[];
}

/** How long typed letters count as one word. */
const TYPE_AHEAD_MS = 600;
/** The rows drawn beyond the view, on each side: enough that a quick scroll shows no blank. */
const OVERSCAN_ROWS = 12;

export function FileTree({ door, directories: dirs, onNavigate, root, label, expanded, onExpandedChange, selected, focused, onFocused, onSelect, onOpen, menuFor, onKey, onDropOn, uploadOver, onRetry, emptyEntries, onRowsChange }: FileTreeProps) {
  const { t } = useTranslation();
  const rows = useMemo(() => layoutRows(root, expanded, dirs, door.environmentId), [root, expanded, dirs, door.environmentId]);
  const scroller = useRef<HTMLDivElement>(null);
  const rowHeight = useRowHeight();
  const typed = useRef({ text: '', at: 0 });
  const focusedRow = useRef<{ id: string; parent: string } | null>(null);
  const pendingFocus = useRef<string | null>(null);

  // The pane owns subscriptions and pages; this virtualized tree only presents its resident rows.
  const opened = useMemo(() => [root, ...rows.filter((row) => row.expandable && row.expanded).map((row) => row.id)], [root, rows]);
  useLayoutEffect(() => { onRowsChange?.(rows); }, [onRowsChange, rows]);

  // The virtualizer hands out functions that change with the list; this component uses them only here.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => rowHeight,
    overscan: OVERSCAN_ROWS,
    getItemKey: (index) => rows[index]?.id ?? index,
  });

  // A replacement page starts at its first row. Live edits within the same page keep the
  // person's scroll position; an old page's bottom offset must not hide the new first sibling.
  const priorPages = useRef<{ root: string; pages: ReadonlyMap<string, string | null> } | null>(null);
  useLayoutEffect(() => {
    const pages = new Map([...dirs].map(([path, window]) => [path, window.page?.incoming ?? null]));
    const before = priorPages.current;
    priorPages.current = { root, pages };
    if (before === null) return;
    if (before.root !== root || before.pages.get(directoryKey(door.environmentId, root)) !== pages.get(directoryKey(door.environmentId, root))) {
      virtual.scrollToOffset(0); return;
    }
    for (const [path, cursor] of pages) if (before.pages.has(path) && before.pages.get(path) !== cursor) {
      const index = rows.findIndex((row) => directoryKey(door.environmentId, row.parent) === path);
      if (index >= 0) {
        virtual.scrollToIndex(index, { align: 'start' });
        const first = rows[index];
        if (first !== undefined && focusedRow.current?.id === `${first.parent}\u0000page`) {
          pendingFocus.current = first.id; onFocused(first.id);
        }
      }
      break;
    }
  }, [dirs, door.environmentId, onFocused, root, rows, virtual]);

  const indexOf = useCallback((id: string | null) => (id === null ? -1 : rows.findIndex((row) => row.id === id)), [rows]);
  const focusIndex = indexOf(focused);
  const tabbable = focusIndex >= 0 ? focused : (rows.find(focusableFileRow)?.id ?? null);

  useLayoutEffect(() => {
    const before = focusedRow.current;
    if (before === null || rows.some((row) => row.id === before.id && focusableFileRow(row))) return;
    const active = document.activeElement;
    if (active !== document.body && active !== null && scroller.current?.contains(active) !== true) { focusedRow.current = null; return; }
    const next = rows.find((row) => row.id === before.parent && focusableFileRow(row))
      ?? rows.find((row) => row.parent === before.parent && focusableFileRow(row)) ?? rows.find(focusableFileRow);
    if (next === undefined) { scroller.current?.focus(); focusedRow.current = null; return; }
    pendingFocus.current = next.id; onFocused(next.id);
    virtual.scrollToIndex(rows.indexOf(next), { align: 'auto' });
  }, [onFocused, rows, virtual]);

  // The row the keyboard moved to is brought into view and given the focus.
  useEffect(() => {
    const id = pendingFocus.current;
    if (id === null) return;
    const index = indexOf(id);
    if (index < 0) return;
    virtual.scrollToIndex(index, { align: 'auto' });
    // Synthetic control-row identities contain a separator that CSS.escape replaces. Match
    // the bounded rendered rows directly so those keyboard targets remain addressable too.
    const element = [...scroller.current?.querySelectorAll<HTMLElement>('[data-row-id]') ?? []].find((row) => row.dataset.rowId === id);
    if (element !== undefined) {
      element.focus();
      pendingFocus.current = null;
    }
  });

  const moveTo = (id: string | undefined) => {
    if (id === undefined) return;
    pendingFocus.current = id;
    onFocused(id);
    virtual.scrollToIndex(indexOf(id), { align: 'auto' });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (isComposing(event.nativeEvent) || event.altKey) return;
    const target = event.target as HTMLElement;
    const item = target.closest<HTMLElement>('[data-row-id]');
    if (item === null) return;
    if (target !== item && !['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
    const index = indexOf(item.dataset.rowId ?? null);
    const row = rows[index];
    if (row === undefined) return;
    const done = () => {
      event.preventDefault();
      event.stopPropagation();
    };
    const entry = row.entry;
    if (event.key.length !== 1) typed.current = { text: '', at: 0 };
    if (event.ctrlKey || event.metaKey) {
      const key = event.key.toLowerCase();
      const command = key === 'c' ? 'copy' : key === 'x' ? 'cut' : key === 'v' ? 'paste' : null;
      // Let a browser paste event carry external clipboard files when no Orch rows were copied.
      if (!event.shiftKey && command !== null && entry !== null && command !== 'paste') { onKey(command, entry); done(); }
      return;
    }
    switch (event.key) {
      case 'ArrowDown':
        moveTo(nextEntry(rows, index, 1)?.id);
        done();
        return;
      case 'ArrowUp':
        moveTo(nextEntry(rows, index, -1)?.id);
        done();
        return;
      case 'Home':
        moveTo(rows.find(focusableFileRow)?.id);
        done();
        return;
      case 'End':
        moveTo([...rows].reverse().find(focusableFileRow)?.id);
        done();
        return;
      case 'ArrowRight':
        if (entry === null) { item.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus(); done(); return; }
        if (row.expandable && !row.expanded) onExpandedChange(entry.path, true);
        else if (row.expandable && row.expanded) {
          const child = nextEntry(rows, index, 1);
          if (child?.parent === row.id) moveTo(child.id);
        }
        done();
        return;
      case 'ArrowLeft':
        if (row.expandable && row.expanded && entry !== null) onExpandedChange(entry.path, false);
        else if (row.parent !== root) moveTo(row.parent);
        done();
        return;
      case 'Enter':
        if (entry === null) {
          if (row.kind === 'page' && row.window !== undefined) onNavigate(row.parent, row.window.canNext ? 'next' : row.window.canPrevious ? 'previous' : 'restart');
          else onRetry(row.parent);
          done(); return;
        }
        if (entry.type === 'dir') onExpandedChange(entry.path, !row.expanded);
        else onOpen(entry, false);
        done();
        return;
      case ' ':
        if (entry !== null) onSelect(entry, event.shiftKey ? 'range' : 'toggle', rows);
        done();
        return;
      case 'F2':
        if (entry !== null) onKey('rename', entry);
        done();
        return;
      case 'Delete':
        if (entry !== null) onKey('delete', entry);
        done();
        return;
      default:
    }
    if (event.key.length === 1 && !event.shiftKey) {
      const now = Date.now();
      const text = (now - typed.current.at > TYPE_AHEAD_MS ? '' : typed.current.text) + event.key.toLocaleLowerCase();
      typed.current = { text, at: now };
      const from = text.length === 1 ? index + 1 : index;
      for (let step = 0; step < rows.length; step += 1) {
        const candidate = rows[(from + step) % rows.length];
        if (candidate?.entry?.name.toLocaleLowerCase().startsWith(text) === true) {
          moveTo(candidate.id);
          done();
          return;
        }
      }
    }
  };

  // A tree holds items and nothing else: while there are none (a directory being read, or empty) it is a plain group.
  const entryCount = rows.reduce((count, row) => (row.kind === 'entry' ? count + 1 : count), 0);
  const hasItems = rows.some(focusableFileRow);
  const waiting = opened.filter((path) => dirs.get(directoryKey(door.environmentId, path))?.watch === 'waiting').length;
  return (
    <>
      <div
        ref={scroller}
        role={hasItems ? 'tree' : 'group'}
        aria-label={label}
        aria-multiselectable={hasItems ? true : undefined}
        className="@container flex min-h-0 flex-1 flex-col overflow-auto"
        onKeyDown={onKeyDown}
        data-testid="file-tree"
        tabIndex={hasItems ? -1 : 0}
        onFocusCapture={(event) => {
          const item = (event.target as HTMLElement).closest<HTMLElement>('[data-row-id]');
          const row = item === null ? undefined : rows.find((row) => row.id === item.dataset.rowId);
          if (row !== undefined) focusedRow.current = { id: row.id, parent: row.parent };
        }}
        onBlurCapture={(event) => {
          if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) focusedRow.current = null;
        }}
      >
        <div className="shrink-0" style={{ height: virtual.getTotalSize(), position: 'relative', width: '100%' }}>
          {virtual.getVirtualItems().map((item) => {
            const row = rows[item.index];
            if (row === undefined) return null;
            return (
              <div key={item.key} style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: item.size, transform: `translateY(${item.start}px)` }}>
                <TreeRow
                  row={row}
                  door={door}
                  tabbable={row.id === tabbable}
                  isSelected={row.entry !== null && selected.has(row.entry.path)}
                  selected={selected}
                  rows={rows}
                  onExpandedChange={onExpandedChange}
                  onFocused={onFocused}
                  onSelect={onSelect}
                  onOpen={onOpen}
                  menuFor={menuFor}
                  onDropOn={onDropOn}
                  uploadOver={uploadOver}
                  onRetry={onRetry}
                  onNavigate={onNavigate}
                />
              </div>
            );
          })}
        </div>
        {emptyEntries !== undefined && (
          <ContextMenu label={t('files.menu.labelEmpty')} entries={emptyEntries}>
            <div className="min-h-10 flex-1" aria-hidden />
          </ContextMenu>
        )}
      </div>
      {waiting > 0 && <p className="shrink-0 px-2 py-1 text-xs text-text-muted" role="status">{t('files.tree.waiting', { count: waiting })}</p>}
      {/* Beside the tree and not in it: a tree holds items and nothing else for a screen reader. */}
      <p className="sr-only" role="status" aria-live="polite">
        {t('files.tree.count', { count: entryCount })}
      </p>
    </>
  );
}

/** The next row that is a file or a directory, in a direction. */
function nextEntry(rows: readonly FileRow[], from: number, step: 1 | -1): FileRow | undefined {
  for (let index = from + step; index >= 0 && index < rows.length; index += step) {
    const row = rows[index];
    if (row !== undefined && focusableFileRow(row)) return row;
  }
  return undefined;
}

interface RowProps {
  row: FileRow;
  door: Door;
  tabbable: boolean;
  isSelected: boolean;
  selected: ReadonlySet<string>;
  rows: readonly FileRow[];
  onExpandedChange: FileTreeProps['onExpandedChange'];
  onFocused: FileTreeProps['onFocused'];
  onSelect: FileTreeProps['onSelect'];
  onOpen: FileTreeProps['onOpen'];
  menuFor: FileTreeProps['menuFor'];
  onDropOn: FileTreeProps['onDropOn'];
  uploadOver: FileTreeProps['uploadOver'];
  onRetry: FileTreeProps['onRetry'];
  onNavigate: FileTreeProps['onNavigate'];
}

/** A row is drawn again only when what it shows changes: a scroll brings new rows in and leaves the others as they are. */
const TreeRow = memo(function TreeRow({ row, door, tabbable, isSelected, selected, rows, onExpandedChange, onFocused, onSelect, onOpen, menuFor, onDropOn, uploadOver, onRetry, onNavigate }: RowProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const errorText = useErrorText();
  const indent = { paddingInlineStart: `calc(var(--spacing-4) * ${row.depth} + var(--spacing-1))` };
  const [over, setOver] = useState(false);
  const failure = row.kind === 'capacity' ? t('files.pages.capacity') : row.reason ?? (row.error === undefined ? t('files.tree.failed') : errorText(row.error).message);

  if (row.kind !== 'entry' || row.entry === null) {
    return (
      <div role={focusableFileRow(row) ? 'treeitem' : 'none'} data-row-id={row.id}
        data-upload-directory={row.kind === 'empty' ? row.parent : undefined}
        aria-level={focusableFileRow(row) ? row.depth + 1 : undefined}
        aria-label={row.kind === 'page' ? t('files.pages.label', { path: row.parent || '/' }) : undefined}
        tabIndex={focusableFileRow(row) && tabbable ? 0 : -1} onFocus={() => onFocused(row.id)}
        style={indent} className="focus-inset flex h-full items-center gap-2 pr-2 text-sm text-text-muted">
        <span className="size-4 shrink-0" aria-hidden />
        {row.kind === 'loading' && <span>{t('common.loading')}</span>}
        {row.kind === 'empty' && <span>{t('files.tree.empty')}</span>}
        {row.kind === 'more' && <span>{t('files.tree.more')}</span>}
        {row.kind === 'page' && row.window !== undefined && <DirectoryPager window={row.window} path={row.parent} navigate={onNavigate} />}
        {(row.kind === 'failed' || row.kind === 'changed' || row.kind === 'capacity') && (
          <>
            <span className="min-w-0 truncate" title={row.kind === 'changed' ? t('files.tree.changed') : failure}>
              {row.kind === 'changed' ? t('files.tree.changed') : failure}
            </span>
            {row.retryable !== false && <Button size="sm" variant="ghost" onClick={() => onRetry(row.parent)}>
              {row.kind === 'changed' ? t('files.tree.reload') : t('common.tryAgain')}
            </Button>}
          </>
        )}
      </div>
    );
  }

  const entry = row.entry;
  const isDir = entry.type === 'dir';
  const Icon = isDir ? (row.expanded ? FolderOpen : Folder) : entry.type === 'symlink' ? FileSymlink : FileIcon;
  // In a cached view the files Orch keeps a copy of are the ones that can still be opened.
  const kept = row.cachedView && !isDir && entry.cached;

  const click = (event: MouseEvent<HTMLElement>) => {
    onFocused(entry.path);
    if (event.shiftKey) onSelect(entry, 'range', rows);
    else if (event.ctrlKey || event.metaKey) onSelect(entry, 'toggle', rows);
    else {
      onSelect(entry, 'single', rows);
      if (isDir) onExpandedChange(entry.path, !row.expanded);
      else onOpen(entry, false);
    }
  };

  const element = (
    <div
      role="treeitem"
      data-row-id={row.id}
      data-path={entry.path}
      data-upload-directory={isDir ? entry.path : undefined}
      aria-level={row.depth + 1}
      aria-posinset={row.setSize < 0 ? undefined : row.position}
      aria-setsize={row.setSize}
      aria-expanded={row.expandable ? row.expanded : undefined}
      aria-selected={isSelected}
      title={[entry.path, isDir ? '' : formatBytes(entry.size, format.locale), formatDateTime(entry.mtime, format)].filter(Boolean).join(' · ')}
      tabIndex={tabbable ? 0 : -1}
      style={indent}
      draggable
      onDragStart={(event) => {
        const paths = isSelected ? [...selected] : [entry.path];
        event.dataTransfer.setData('application/x-orch-paths', workspaceDrag(door.environmentId, paths));
        const reference = paths.map((path) => environmentFileReference(door.environmentId, path)).join('\n');
        event.dataTransfer.setData(ORCH_REFERENCE, reference);
        event.dataTransfer.setData('text/plain', reference);
        event.dataTransfer.effectAllowed = 'copyMove';
      }}
      onDragOver={(event) => {
        if (!isDir || onDropOn === undefined || !event.dataTransfer.types.includes('application/x-orch-paths')) return;
        event.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(event) => {
        setOver(false);
        if (!isDir || onDropOn === undefined) return;
        const raw = event.dataTransfer.getData('application/x-orch-paths');
        if (raw === '') return;
        event.preventDefault();
        event.stopPropagation();
        const payload = readWorkspaceDrag(raw, door.environmentId);
        if (payload.kind === 'local') onDropOn(entry, payload.paths);
        else if (payload.kind === 'foreign') toast({ tone: 'info', title: t('files.menu.otherWorkspace') });
      }}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocused(entry.path);
      }}
      onContextMenu={() => { onFocused(entry.path); if (!isSelected) onSelect(entry, 'single', rows); }}
      onClick={click}
      onDoubleClick={() => {
        if (!isDir) onOpen(entry, true);
      }}
      className={cx(
        'group/row focus-inset relative flex h-full cursor-default items-center gap-1 pr-1 text-base select-none',
        isSelected ? 'bg-accent-subtle text-text' : 'text-text hover:bg-bg-muted',
        (over || uploadOver === entry.path) && 'outline-2 -outline-offset-2 outline-accent',
      )}
    >
      <span aria-hidden className={cx('flex size-4 shrink-0 items-center justify-center rounded-sm text-text-subtle', !row.expandable && 'invisible')}>
        <ChevronRight className={cx('transition-transform', row.expanded && 'rotate-90')} />
      </span>
      {isDir || entry.type === 'symlink' ? <><Icon className={cx('shrink-0', isDir ? 'text-accent-text' : 'text-text-muted')} aria-hidden /><span data-user-content className="min-w-0 flex-1 truncate">{entry.name}</span></> :
        <span className="min-w-0 flex-1"><FileChip source={{ ...workspaceSource(door, entry.path, entry.name, entry.size), updatedAt: entry.mtime }} presentation={{ place: 'compact' }} bare onOpen={click} menuEntries={() => menuFor(entry, selected.has(entry.path) ? selected : new Set([entry.path]))} /></span>}
      {over && <span role="status" className="min-w-0 truncate text-sm text-text-muted">{t('uploads.target.move', { path: entry.path })}</span>}
      {kept && <span role="img" aria-label={t('files.tree.cached')} title={t('files.tree.cached')} className="size-2 shrink-0 rounded-full bg-info" />}
      <MenuButton entry={entry} menuFor={menuFor} selected={selected} />
    </div>
  );

  return (
    <ContextMenu
      label={t('files.menu.label', { name: entry.name })}
      content={<RowMenu entry={entry} menuFor={menuFor} selected={selected} />}
      onOpenChange={() => undefined}
    >
      {element}
    </ContextMenu>
  );
});

/** The entries of a row's menu, made when the menu opens. */
function RowMenu({ entry, menuFor, selected }: { entry: DirEntry; menuFor: FileTreeProps['menuFor']; selected: ReadonlySet<string> }) {
  const entries = useMemo(() => menuFor(entry, selected.has(entry.path) ? selected : new Set([entry.path])), [entry, menuFor, selected]);
  return <ContextEntries entries={entries} />;
}

/** The "⋯" of a row: the same list as the right mouse button's, for touch and for those who do not right-click. */
function MenuButton({ entry, menuFor, selected }: { entry: DirEntry; menuFor: FileTreeProps['menuFor']; selected: ReadonlySet<string> }) {
  const { t } = useTranslation();
  return (
    <Menu
      label={t('files.menu.label', { name: entry.name })}
      trigger={
        <button
          type="button"
          aria-label={t('files.menu.for', { name: entry.name })}
          tabIndex={-1}
          onClick={(event) => event.stopPropagation()}
          className="focus-inset invisible inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted group-focus-within/row:visible group-hover/row:visible group-aria-selected/row:visible hover:bg-border hover:text-text"
        >
          <MoreHorizontal aria-hidden />
        </button>
      }
      content={<MenuContent entry={entry} menuFor={menuFor} selected={selected} />}
    />
  );
}

function MenuContent({ entry, menuFor, selected }: { entry: DirEntry; menuFor: FileTreeProps['menuFor']; selected: ReadonlySet<string> }) {
  const entries = useMemo(() => menuFor(entry, selected.has(entry.path) ? selected : new Set([entry.path])), [entry, menuFor, selected]);
  return <DropdownEntries entries={entries} />;
}
