/** The workspace of the thread's environment, opened at the thread's directory, with who else works there; the search by name and inside files; the tree's context menu; live or cached; live only for a tree too large to index.
 *
 * The files of a thread in a tree that is read as it is opened and kept equal to the disk. The path
 * heads the panel, with the way up to the rest of the workspace beside it; where other threads work
 * in the same environment one line says so, since their files are here too. A banner says whether
 * what is shown is read from the environment now or from its last list, and since when. A file opens in
 * the code view as a tab beside the conversation; making, renaming, copying, moving and deleting are in
 * the menu of a row and in the toolbar, for whoever may write the thread's files, and switched off with
 * the reason for those who may not.
 */
import { ArrowUp, ChevronsDownUp, FilePlus, FolderPlus, Folder as FolderIcon, Info, MoreHorizontal, RefreshCw, Search, X } from 'lucide-react';
import { useCallback, useLayoutEffect, useRef, useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useViewInstance } from '@/features/workbench/instance';
import { useTranslation } from 'react-i18next';

import type { DirEntry, Environment, EnvironmentIndexReason } from '@/generated/api-types';
import { directoryKey } from '@/realtime/stores/environment';
import type { ViewProps } from '@/features/workbench/views';
import { formatDateTime } from '@/lib/time';
import { useFormatContext } from '@/lib/appearance';
import { Banner } from '@/ui/Banner';
import { EmptyState } from '@/ui/EmptyState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Tooltip } from '@/ui/Tooltip';
import { toast } from '@/ui/Toast';
import { SkeletonRows } from '@/ui/Skeleton';
import { carriesFiles, draggedFileCount } from '@/lib/drop';
import { useFileDrops } from '@/features/composer/FileDrops';
import { UploadHelp } from '@/features/composer/UploadHelp';
import { formatNumber } from '@/lib/format';
import { cx } from '@/ui/cx';
import { StatusDot } from '@/ui/StatusDot';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';

import { useEnvironment, usePanelWorkspaceDoor, useRouteThread, type EnvironmentWorkspaceAccess, workspaceBinding, workspaceDenial } from '../files/door';
import { useDirectoryTree } from '../files/useDirectoryTree';
import { DirectoryPager } from '../files/DirectoryPager';
import { FileTree, focusableFileRow, type FileRow } from '../files/FileTree';
import { openFile, parentOf } from '../files/FileTabs';
import { useFileActions, whyNoWrite } from '../files/FileMenu';
import { NO_SEARCH, SearchBox, SearchResults, type SearchState } from '../files/SearchInFiles';

/** The Files panel's reason for a live tree, with an honest fallback for an older connector. */
export function liveIndexReasonKey(reason: EnvironmentIndexReason | null | undefined): 'largeTree.reasons.chosen' | 'largeTree.reasons.entries' | 'largeTree.reasons.watches' | 'largeTree.reasons.scan_time' | 'largeTree.reasons.unknown' {
  switch (reason) {
    case 'chosen': return 'largeTree.reasons.chosen';
    case 'entries': return 'largeTree.reasons.entries';
    case 'watches': return 'largeTree.reasons.watches';
    case 'scan_time': return 'largeTree.reasons.scan_time';
    default: return 'largeTree.reasons.unknown';
  }
}

interface RetainedSource {
  reader: number | undefined;
  signature: string;
  key: string;
  // Presentation/source only. No Thread row or permission survives an admission gap.
  source: Pick<EnvironmentWorkspaceAccess, 'door' | 'directory' | 'environment'>;
}

/** The panel in the view's place: it needs a thread to open the workspace through, and the permission to look. */
export function FilesView(_props: ViewProps) {
  const { t } = useTranslation();
  const doorState = usePanelWorkspaceDoor();
  const instance = useViewInstance(), routeThread = useRouteThread();
  const reader = useStore(client.session, (state) => state.readerVersion);
  const [retained, setRetained] = useState<RetainedSource | null>(null);
  const current = doorState.kind === 'ready' && doorState.access.mayView ? doorState.access : null;
  const source = current === null ? null : { door: current.door, directory: current.directory, environment: current.environment };
  const signature = source === null ? null : JSON.stringify([reader, source]);
  const target = instance?.target;
  const compatible = retained !== null && retained.reader === reader && (target?.workspace !== undefined
    ? target.workspace.environmentId === retained.source.door.environmentId && target.workspace.threadId === retained.source.door.threadId
    : target?.kind === 'environment' ? target.id === retained.source.door.environmentId && retained.source.door.threadId === null
      : routeThread !== null && routeThread === retained.source.door.threadId);
  const pending = current === null && doorState.kind === 'loading' && doorState.admission === true && compatible;
  let held = retained;
  if (current !== null && source !== null && signature !== null && retained?.signature !== signature) {
    held = { reader, signature, source, key: JSON.stringify([reader, current.door, current.directory]) };
    setRetained(held);
  } else if (current === null && !pending && retained !== null) {
    held = null; setRetained(null);
  }
  // A temporary same-source admission must not destroy the mounted ledger-owned tree.
  // It is never an authority supplier: a paused body receives no Thread or permissions.
  const presentation = current !== null ? { access: current, key: JSON.stringify([reader, current.door, current.directory]) }
    : pending && held !== null ? { access: { ...held.source, thread: null, mayView: false, mayManage: false, mayWatchProcesses: false, mayShell: false }, key: held.key } : null;
  if (presentation !== null) {
    return <>
      {pending && <SkeletonRows rows={8} />}
      <div className="h-full min-h-0" hidden={pending} inert={pending} aria-hidden={pending ? true : undefined}
        onClickCapture={(event) => { if (pending) { event.preventDefault(); event.stopPropagation(); } }}
        onKeyDownCapture={(event) => { if (pending) { event.preventDefault(); event.stopPropagation(); } }}>
        <FilesPanel key={presentation.key} access={presentation.access} paused={pending} />
      </div>
    </>;
  }
  if (doorState.kind === 'error') return <ErrorState error={doorState.error} />;
  if (doorState.kind === 'loading') return <SkeletonRows rows={8} />;
  if (doorState.kind === 'none') {
    return (
      <EmptyState
        compact
        className="h-full"
        icon={<FolderIcon aria-hidden />}
        title={doorState.why === 'no_thread' ? t('files.view.noThread') : t('files.view.noEnvironment')}
        description={doorState.why === 'no_thread' ? t('files.view.noThreadAbout') : t('files.view.noEnvironmentAbout')}
      />
    );
  }
  const { access } = doorState;
  if (!access.mayView) {
    const missing = workspaceDenial(t, 'files.view', access.environment);
    return <EmptyState compact className="h-full" icon={<FolderIcon aria-hidden />} title={missing} />;
  }
  return null;
}

export function FilesPanel({ access, paused = false }: { access: EnvironmentWorkspaceAccess; paused?: boolean }) {
  const reader = useStore(client.session, (state) => state.readerVersion);
  const { environment, failed } = useEnvironment(access.door.environmentId);
  const binding = environment === undefined ? null : workspaceBinding(environment);
  const key = JSON.stringify([reader, access.door, access.directory, binding]);
  return <FilesPanelContents key={key} access={access} environment={environment} failed={failed} binding={binding} paused={paused} />;
}

function FilesPanelContents({ access, environment, failed, binding, paused }: {
  access: EnvironmentWorkspaceAccess; environment: Environment | undefined; failed: unknown; binding: string | null; paused: boolean;
}) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { door } = access;
  const fileDrops = useFileDrops(`workspace:${JSON.stringify(door)}`);
  const current = useRef(false);
  useLayoutEffect(() => {
    current.current = !paused;
    return () => { current.current = false; };
  }, [paused]);
  const [admitted, setAdmitted] = useState(!paused);
  if (!paused && !admitted) setAdmitted(true);
  const [root, setRoot] = useState(access.directory);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [focused, setFocused] = useState<string | null>(null);
  const [dropDirectory, setDropDirectory] = useState<string | null>(null);
  const [dropCount, setDropCount] = useState<number | null>(null);
  const [search, setSearch] = useState<SearchState>(NO_SEARCH);
  const [searchOpen, setSearchOpen] = useState(false);
  const searching = searchOpen && search.text.trim() !== '';
  const { tree, directories, expanded, capacity } = useDirectoryTree(door, root, binding, admitted);
  const anchor = useRef<string | null>(null);
  const rowsNow = useRef<readonly FileRow[]>([]);
  const acceptRows = useCallback((rows: readonly FileRow[]) => {
    rowsNow.current = rows;
    const paths = new Set(rows.flatMap((row) => row.entry === null ? [] : [row.entry.path]));
    setSelected((before) => [...before].every((path) => paths.has(path)) ? before : new Set([...before].filter((path) => paths.has(path))));
    const focus = new Set(rows.filter(focusableFileRow).map((row) => row.id));
    setFocused((before) => before === null || focus.has(before) ? before : rows.find(focusableFileRow)?.id ?? null);
    if (anchor.current !== null && !paths.has(anchor.current)) anchor.current = null;
  }, []);
  const directory = directories.get(directoryKey(door.environmentId, root));
  const allLive = capacity === null && directories.size > 0 && [...directories.values()].every((one) => one.watch === 'live');
  const focusedDirectory = directories.get(directoryKey(door.environmentId, focused === null ? root : parentOf(focused)));
  const focusedEntry = focusedDirectory?.entries.find((entry) => entry.path === focused);

  const open = useCallback(
    (entry: DirEntry, permanent: boolean) => { if (current.current) openFile({ environmentId: door.environmentId, threadId: door.threadId ?? '', path: entry.path, permanent }); },
    [door.environmentId, door.threadId],
  );

  const actions = useFileActions({
    access: paused ? { ...access, mayView: false, mayManage: false, mayWatchProcesses: false, mayShell: false } : access,
    environment,
    open,
    collapseAll: () => tree.collapse(),
    search: (under) => { setSearch({ ...NO_SEARCH, mode: 'content', under }); setSearchOpen(true); },
    entries: () => rowsNow.current.flatMap((row) => (row.entry === null ? [] : [row.entry])),
  });

  const select = useCallback((entry: DirEntry, how: 'single' | 'toggle' | 'range', rows: readonly FileRow[]) => {
    if (!current.current) return;
    rowsNow.current = rows;
    if (how === 'single') {
      anchor.current = entry.path;
      setSelected(new Set([entry.path]));
    } else if (how === 'toggle') {
      anchor.current = entry.path;
      setSelected((before) => {
        const next = new Set(before);
        if (next.has(entry.path)) next.delete(entry.path);
        else next.add(entry.path);
        return next;
      });
    } else {
      const paths = rows.flatMap((row) => (row.entry === null ? [] : [row.entry.path]));
      const from = paths.indexOf(anchor.current ?? entry.path);
      const to = paths.indexOf(entry.path);
      if (from >= 0 && to >= 0) setSelected(new Set(paths.slice(Math.min(from, to), Math.max(from, to) + 1)));
    }
  }, []);

  const folderOfFocus = (): string => {
    if (focusedEntry === undefined) return root;
    return focusedEntry.type === 'dir' ? focusedEntry.path : parentOf(focusedEntry.path);
  };

  const openFolder = (path: string) => {
    if (!current.current) return;
    setSearch(NO_SEARCH);
    setSearchOpen(false);
    setRoot(path); setSelected(new Set()); setFocused(null); anchor.current = null;
  };

  const refresh = () => { if (current.current) tree.refresh(); };

  const menuFor = useCallback(
    (entry: DirEntry, chosen: ReadonlySet<string>): MenuEntry[] => [
      ...(entry.type === 'dir' ? [{ id: 'open-here', label: t('files.pages.openHere'), icon: <FolderIcon aria-hidden />, onSelect: () => openFolder(entry.path) }] : []),
      ...actions.entriesFor(entry, chosen, rowsNow.current.flatMap((row) => (row.entry === null ? [] : [row.entry]))),
    ],
    // The menu is made when it opens, from the actions of that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [actions.entriesFor, t],
  );

  const why = whyNoWrite(access, environment, t);
  const others = environment === undefined ? 0 : environment.threads.filter((one) => one.thread.id !== door.threadId).length + environment.threads_hidden;
  const liveOnly = environment?.index === 'live';
  const cached = directory?.source === 'cached';
  const asOf = directory?.asOf ?? null;
  const heading = root === '' ? access.environment.name : t('files.panel.heading', { directory: `${root}/`, environment: access.environment.name });
  const showSearch = () => { if (liveOnly) setSearch((before) => ({ ...before, mode: 'content' })); setSearchOpen(true); };
  const panelEntries: MenuEntry[] = [
    ...actions.emptyEntries(folderOfFocus()).map((entry) => entry.id === 'refresh' ? { id: 'refresh', label: t('files.panel.refresh'), icon: <RefreshCw aria-hidden />, onSelect: refresh } : entry),
    { id: 'search', label: t('files.search.label'), icon: <Search aria-hidden />, onSelect: showSearch },
    ...(root === '' ? [] : [{ id: 'up', label: t('files.panel.up'), icon: <ArrowUp aria-hidden />, onSelect: () => openFolder(parentOf(root)) }]),
    ...(root === access.directory ? [] : [{ id: 'thread-directory', label: t('files.panel.backToDirectory'), icon: <FolderIcon aria-hidden />, onSelect: () => openFolder(access.directory) }]),
  ];

  if (environment === undefined && failed !== null && failed !== undefined) return <ErrorState compact error={failed} />;
  if (directory?.watch === 'refused') return <ErrorState compact error={directory.error} />;

  const folderUnderPointer = (target: EventTarget): string | null => {
    if (!(target instanceof HTMLElement) || target.closest('[data-testid="file-tree"]') === null) return null;
    const row = target.closest<HTMLElement>('[data-row-id]');
    return row === null ? root : row.dataset.uploadDirectory ?? null;
  };

  return (
    <div
      className={cx('relative flex h-full min-h-0 flex-col', dropDirectory === root && 'outline-2 -outline-offset-2 outline-accent')}
      data-testid="files-panel"
      onDragOver={(event) => {
        if (!current.current || !carriesFiles(event.dataTransfer)) return;
        const target = folderUnderPointer(event.target);
        setDropCount(draggedFileCount(event.dataTransfer));
        setDropDirectory(why === false ? target : null);
        if (target !== null && why === false) { event.preventDefault(); event.stopPropagation(); event.dataTransfer.dropEffect = 'copy'; }
      }}
      onDragLeave={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setDropDirectory(null); }}
      onDrop={(event) => {
        setDropDirectory(null);
        if (!current.current || !carriesFiles(event.dataTransfer)) return;
        event.preventDefault(); event.stopPropagation();
        const target = folderUnderPointer(event.target);
        if (target === null) return;
        if (why !== false) { toast({ tone: 'info', title: why }); return; }
        fileDrops.read(event.dataTransfer, t('uploads.target.folder', { path: target || '/' }), (files) => actions.upload(target, files));
      }}
      onPaste={(event) => {
        if (!current.current || event.target instanceof HTMLElement && event.target.closest('input,textarea,[contenteditable="true"]') !== null) return;
        if (carriesFiles(event.clipboardData)) {
          event.preventDefault(); event.stopPropagation();
          if (why !== false) { toast({ tone: 'info', title: why }); return; }
          const folder = folderOfFocus();
          fileDrops.read(event.clipboardData, t('uploads.target.folder', { path: folder || '/' }), (files) => actions.upload(folder, files));
        } else {
          const entry = actions.emptyEntries(folderOfFocus()).find((one) => one.id === 'paste');
          if (entry !== undefined && 'onSelect' in entry && entry.disabled === false) { event.preventDefault(); entry.onSelect(); }
        }
      }}
    >
      {dropDirectory !== null && <p role="status" className="pointer-events-none absolute inset-x-1 top-10 z-10 truncate rounded-md border border-border bg-surface px-2 py-1 text-sm text-text-muted shadow-sm">{t(dropCount === null ? 'uploads.target.folder' : 'uploads.target.folderCount', { path: dropDirectory || '/', count: dropCount ?? undefined, number: dropCount === null ? undefined : formatNumber(dropCount, format.locale) })}</p>}
      <div className="flex shrink-0 items-center gap-0.5 border-b border-border px-2 py-1">
        <h2 className="min-w-0 flex-1 truncate text-base font-medium text-text" title={heading}>
          {t('files.panel.inEnvironment', { name: heading })}
        </h2>
        <UploadHelp topic="workspace-uploads" />
        <IconButton size="sm" label={t('files.menu.newFile')} icon={<FilePlus aria-hidden />} blocked={why !== false} reason={why === false ? undefined : why} onClick={() => actions.start({ kind: 'newFile', directory: folderOfFocus() })} />
        <IconButton size="sm" label={t('files.menu.newFolder')} icon={<FolderPlus aria-hidden />} blocked={why !== false} reason={why === false ? undefined : why} onClick={() => actions.start({ kind: 'newFolder', directory: folderOfFocus() })} />
        <IconButton size="sm" label={t('files.panel.refresh')} icon={<RefreshCw aria-hidden />} onClick={refresh} />
        <IconButton size="sm" label={t('files.menu.collapseAll')} icon={<ChevronsDownUp aria-hidden />} onClick={() => tree.collapse()} />
        <IconButton size="sm" label={t('files.search.label')} icon={<Search aria-hidden />} pressed={searchOpen} onClick={() => searchOpen ? setSearchOpen(false) : showSearch()} />
        <Menu label={t('files.menu.labelEmpty')} entries={panelEntries} trigger={<IconButton size="sm" label={t('files.menu.labelEmpty')} icon={<MoreHorizontal aria-hidden />} />} />
      </div>
      {cached && (
        <Banner tone="warning">
          {asOf === null ? t('files.panel.cachedNoTime') : t('files.panel.cached', { time: formatDateTime(asOf, format) })}
        </Banner>
      )}
      {searchOpen && <div className="shrink-0 border-b border-border pt-1" onKeyDown={(event) => { if (event.key === 'Escape') { event.stopPropagation(); setSearchOpen(false); } }}>
        <div className="flex items-start"><div className="min-w-0 flex-1"><SearchBox state={search} onChange={setSearch} namesAvailable={!liveOnly} directory={root} autoFocus /></div><IconButton size="sm" label={t('files.search.close')} icon={<X aria-hidden />} onClick={() => setSearchOpen(false)} /></div>
      </div>}
      {why !== false && <p className="px-2 py-1 text-xs text-text-muted" data-testid="why-read-only">{why}</p>}
      {(capacity !== null || directory?.capacity !== null && directory?.capacity !== undefined) && <div className="shrink-0 border-b border-border px-2 py-1 text-xs text-text-muted" role="status">
        <p>{t('files.pages.capacity')}</p>
        {capacity !== null && capacity.path !== root
          ? <Button size="sm" variant="ghost" onClick={() => openFolder(capacity.path)}>{t('files.pages.openHere')}</Button>
          : <Button size="sm" variant="ghost" onClick={() => tree.navigate(root, 'restart')}>{t('common.tryAgain')}</Button>}
      </div>}
      {searching ? (!paused &&
        <SearchResults
          door={door}
          state={search}
          directory={root}
          onOpenPath={(path, line) => { if (current.current) openFile({ environmentId: door.environmentId, threadId: door.threadId ?? '', path, line, permanent: false }); }}
          onOpenFolder={openFolder}
        />
      ) : capacity?.path === root || directory?.capacity !== undefined && directory.capacity !== null && directory.entries.length === 0 ? (
        <EmptyState compact className="flex-1" icon={<FolderIcon aria-hidden />} title={t('files.pages.unavailable')} />
      ) : (
        <FileTree
          door={door}
          directories={directories}
          onNavigate={(path, action) => { if (current.current) tree.navigate(path, action); }}
          onRowsChange={acceptRows}
          root={root}
          uploadOver={dropDirectory}
          label={t('files.panel.treeLabel', { directory: heading })}
          expanded={expanded}
          onExpandedChange={(path, expanded) => { if (current.current) tree.toggle(path, expanded); }}
          selected={selected}
          focused={focused}
          onFocused={(path) => { if (current.current) setFocused(path); }}
          onSelect={select}
          onOpen={open}
          menuFor={menuFor}
          onKey={(key, entry) => {
            if (!current.current) return;
            const choices = key === 'paste' ? actions.emptyEntries(entry.type === 'dir' ? entry.path : parentOf(entry.path)) : menuFor(entry, selected);
            const choice = choices.find((one) => one.id === key);
            if (choice === undefined || !('onSelect' in choice)) return;
            if (typeof choice.disabled === 'string') { toast({ tone: 'info', title: choice.disabled }); return; }
            choice.onSelect();
          }}
          onDropOn={why === false ? (target, paths) => { if (current.current) actions.move(target, paths); } : undefined}
          onRetry={(path) => { if (current.current) tree.navigate(path, 'restart'); }}
          emptyEntries={actions.emptyEntries(root)}
        />
      )}
      {!paused && <div className="max-h-[30dvh] shrink-0 overflow-auto px-2">{fileDrops.progress}{actions.progress}</div>}
      <div className="flex shrink-0 flex-wrap items-center gap-x-2 border-t border-border px-2 py-1 text-xs text-text-muted">
        <StatusDot tone={allLive ? 'success' : 'idle'} label={allLive ? t('files.panel.live') : t('files.panel.snapshot')} showLabel />
        {directory !== undefined && <DirectoryPager window={directory} path={root} navigate={(path, action) => { if (current.current) tree.navigate(path, action); }} />}
        {others > 0 && <span className="min-w-0 flex-1 truncate" title={t('files.panel.shared', { name: access.environment.name, count: others })}>{t('files.panel.sharedCount', { count: others })}</span>}
        {liveOnly && <Tooltip content={`${t(liveIndexReasonKey(environment.index_reason))} ${t('largeTree.mountSuggestion')}`}><span tabIndex={0} className="ml-auto inline-flex" aria-label={t(liveIndexReasonKey(environment.index_reason))}><Info aria-hidden /></span></Tooltip>}
      </div>
      {!paused && actions.picking}
      {!paused && actions.dialogs}
    </div>
  );
}

