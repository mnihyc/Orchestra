/** The context menus of a file, a directory, several rows, the empty part of the tree, a tab and a selection; cut, copy and paste of rows; an entry one may not use disabled with the reason.
 *
 * Everything that can be done to a thing is in its menu, the same list under the right mouse button
 * as under its "⋯" and `Shift+F10`. The first entry is what a click does; what changes the file
 * follows, then what copies something about it, and what destroys comes last, apart. An entry the
 * person may not use is shown disabled with the reason: a menu that changes shape teaches nothing.
 * The acts that need a word from the person (a name, a place, a yes) open a small dialog of their own.
 */
import { contentHref } from '@/features/workbench/contentAddress';
import { environmentFileReference } from '@/features/people/references';
import { Copy, Download, ExternalLink, FilePlus, FolderPlus, FolderSearch, Link2, MessageSquarePlus, Pencil, RefreshCw, Scissors, Trash2, Upload, ClipboardPaste, CopyPlus, FolderInput, ChevronsDownUp } from 'lucide-react';
import type { TFunction } from 'i18next';
import { type ReactNode, useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { create, useStore } from 'zustand';

import { client } from '@/api/client';
import { copyFile, deleteFile, makeDirectory, renameFile, writeFile, type Door } from '@/api/workspace';
import type { DirEntry, Environment } from '@/generated/api-types';
import { directoryWriter } from '@/realtime/stores/environment';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import type { MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { copyText } from '@/ui/clipboard';

import { addToMessage } from './AddToMessage';
import { type EnvironmentWorkspaceAccess, isAway, workspaceDenial } from './door';
import { downloadFile } from './content';
import { baseName, parentOf } from './FileTabs';
import { factsOf, rememberChoice, viewersFor } from './viewers';
import { useWorkspaceUploads } from './Uploads';

// ---------------------------------------------------------------- cut, copy and paste of rows

interface Clipboard {
  environmentId: string | null;
  mode: 'cut' | 'copy' | null;
  paths: readonly string[];
}

/** What was cut or copied in the tree, until it is pasted or replaced. */
const EMPTY_CLIPBOARD: Clipboard = { environmentId: null, mode: null, paths: [] };
export const useFileClipboard = create<Clipboard>(() => EMPTY_CLIPBOARD);
client.session.subscribe((next, previous) => {
  if (next.readerVersion !== previous.readerVersion) useFileClipboard.setState(EMPTY_CLIPBOARD);
});

// ---------------------------------------------------------------- the acts that ask for a word

export type FileAct =
  | { kind: 'newFile'; directory: string }
  | { kind: 'newFolder'; directory: string }
  | { kind: 'rename'; entry: DirEntry }
  | { kind: 'duplicate'; entry: DirEntry }
  | { kind: 'move'; entries: readonly DirEntry[] }
  | { kind: 'delete'; entries: readonly DirEntry[] };

const joinPath = (directory: string, name: string): string => (directory === '' ? name : `${directory}/${name}`);

/** What a name may not be: empty, or a path of its own. */
export function nameFault(name: string): 'empty' | 'slash' | 'dots' | null {
  const trimmed = name.trim();
  if (trimmed === '') return 'empty';
  if (trimmed.includes('/')) return 'slash';
  if (trimmed === '.' || trimmed === '..') return 'dots';
  return null;
}

/** A directory path typed by a person, cleaned: no leading or trailing slash, no empty parts. */
export function cleanDirectory(text: string): string {
  return text
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part !== '' && part !== '.')
    .join('/');
}

export interface FileActions {
  /** The menu of a file or directory, or of several rows. */
  entriesFor: (entry: DirEntry, selection: ReadonlySet<string>, rows: readonly DirEntry[]) => MenuEntry[];
  /** The menu of the empty part of the tree, or of the tree's own "⋯": the directory it is opened at. */
  emptyEntries: (directory: string) => MenuEntry[];
  /** Starts an act that asks a word of the person. */
  start: (act: FileAct) => void;
  /** Reads files from the desktop into a directory. */
  upload: (directory: string, files: readonly File[]) => void;
  /** Opens the dialog of the desktop's file chooser: what lands in the directory. */
  pick: (directory: string) => void;
  /** Rows dragged onto a directory are moved into it. */
  move: (target: DirEntry, paths: readonly string[]) => void;
  /** Where the dialogs of the acts stand; the owner draws it once. */
  dialogs: ReactNode;
  /** The input that "Upload…" opens. */
  picking: ReactNode;
  /** Pending resumable transfers and their recoverable failures. */
  progress: ReactNode;
}

export interface FileActionOptions {
  access: EnvironmentWorkspaceAccess;
  environment: Environment | undefined;
  /** A file is opened. */
  open: (entry: DirEntry, permanent: boolean) => void;
  /** The tree shows the directory again after something changed in it. */
  collapseAll: () => void;
  search: (directory: string) => void;
  /** Rows of the tree that are in view, for the act on several. */
  entries: () => readonly DirEntry[];
}

/** The reason a person may not change files here, as a sentence; `false` where they may. */
export function whyNoWrite(access: EnvironmentWorkspaceAccess, environment: Environment | undefined, t: TFunction): string | false {
  if (!access.mayManage) return workspaceDenial(t, 'files.manage', access.environment);
  if (environment === undefined) return t('common.loading');
  if (isAway(environment)) return t('files.menu.why.away', { name: access.environment.name });
  if (!environment.remote_write) {
    return environment.machine === null ? t('files.menu.why.noWritesHere') : t('files.menu.why.noWrites', { machine: environment.machine.name });
  }
  return false;
}

export function useFileActions({ access, environment, open, collapseAll, search, entries }: FileActionOptions): FileActions {
  const { t } = useTranslation();
  const { door } = access;
  const userId = useStore(client.session, (state) => state.userId);
  const clipboard = useFileClipboard();
  const [act, setAct] = useState<FileAct | null>(null);
  const picker = useRef<HTMLInputElement>(null);
  const pickInto = useRef('');
  const why = whyNoWrite(access, environment, t);
  const pasteBlocked = why !== false ? why : clipboard.mode === null ? t('files.menu.nothingToPaste') : clipboard.environmentId !== door.environmentId ? t('files.menu.otherWorkspace') : false;
  const start = useCallback((next: FileAct) => {
    if (why !== false) { toast({ tone: 'info', title: why }); return; }
    setAct(next);
  }, [why]);

  const refresh = useCallback(
    async (...directories: string[]) => {
      await Promise.all([...new Set(directories)].map((directory) => directoryWriter.load(door, directory, { source: 'live' }).catch(() => undefined)));
    },
    [door],
  );

  const uploads = useWorkspaceUploads(door, why, refresh);
  const upload = uploads.add;

  const paste = useCallback(
    (directory: string) => {
      const captured = useFileClipboard.getState();
      const { mode, paths, environmentId } = captured;
      if (why !== false) { toast({ tone: 'info', title: why }); return; }
      if (mode === null) return;
      if (environmentId !== door.environmentId) { toast({ tone: 'info', title: t('files.menu.otherWorkspace') }); return; }
      const reader = client.session.getState().readerVersion;
      void (async () => {
        try {
          for (const from of paths) {
            if (client.session.getState().readerVersion !== reader) return;
            const to = joinPath(directory, baseName(from));
            if (mode === 'cut') await renameFile(door, from, to);
            else await copyFile(door, from, to);
          }
          if (client.session.getState().readerVersion !== reader) return;
          if (mode === 'cut' && useFileClipboard.getState() === captured) useFileClipboard.setState(EMPTY_CLIPBOARD);
          await refresh(directory, ...paths.map(parentOf));
        } catch {
          toast({ tone: 'danger', title: t('files.menu.pasteFailed') });
        }
      })();
    },
    [door, refresh, t, why],
  );

  const linkTo = (path: string) => `${window.location.origin}${contentHref({ view: 'file', environmentId: door.environmentId, threadId: door.threadId, path })}`;
  const fromDirectory = (path: string) => (access.directory !== '' && path.startsWith(`${access.directory}/`) ? path.slice(access.directory.length + 1) : path);

  const entriesFor = useCallback(
    (entry: DirEntry, selection: ReadonlySet<string>, rows: readonly DirEntry[]): MenuEntry[] => {
      const several = selection.size > 1 && selection.has(entry.path);
      const chosen = several ? rows.filter((row) => selection.has(row.path)) : [entry];
      const isDir = entry.type === 'dir';
      const facts = factsOf({ name: entry.name, media_type: '', is_text: true, size: entry.size });
      const list: MenuEntry[] = [];
      if (several) {
        list.push(
          { id: 'cut', label: t('files.menu.cut'), icon: <Scissors aria-hidden />, shortcut: 'Mod+X', disabled: why, onSelect: () => useFileClipboard.setState({ environmentId: door.environmentId, mode: 'cut', paths: chosen.map((row) => row.path) }) },
          { id: 'copy', label: t('files.menu.copy'), icon: <Copy aria-hidden />, shortcut: 'Mod+C', disabled: why, onSelect: () => useFileClipboard.setState({ environmentId: door.environmentId, mode: 'copy', paths: chosen.map((row) => row.path) }) },
          { id: 'move', label: t('files.menu.move'), icon: <FolderInput aria-hidden />, disabled: why, onSelect: () => start({ kind: 'move', entries: chosen }) },
          { id: 'sep-danger', type: 'separator' },
          { id: 'delete', label: t('files.menu.deleteMany', { count: chosen.length }), icon: <Trash2 aria-hidden />, danger: true, shortcut: 'Delete', disabled: why, onSelect: () => start({ kind: 'delete', entries: chosen }) },
        );
        return list;
      }
      if (isDir) {
        list.push(
          { id: 'new-file', label: t('files.menu.newFile'), icon: <FilePlus aria-hidden />, disabled: why, onSelect: () => start({ kind: 'newFile', directory: entry.path }) },
          { id: 'new-folder', label: t('files.menu.newFolder'), icon: <FolderPlus aria-hidden />, disabled: why, onSelect: () => start({ kind: 'newFolder', directory: entry.path }) },
          {
            id: 'upload',
            label: t('files.menu.uploadHere'),
            icon: <Upload aria-hidden />,
            disabled: why,
            onSelect: () => {
              pickInto.current = entry.path;
              picker.current?.click();
            },
          },
          { id: 'sep-edit', type: 'separator' },
          { id: 'rename', label: t('files.menu.rename'), icon: <Pencil aria-hidden />, shortcut: 'F2', disabled: why, onSelect: () => start({ kind: 'rename', entry }) },
          { id: 'duplicate', label: t('files.menu.duplicate'), icon: <CopyPlus aria-hidden />, disabled: why, onSelect: () => start({ kind: 'duplicate', entry }) },
          { id: 'move', label: t('files.menu.move'), icon: <FolderInput aria-hidden />, disabled: why, onSelect: () => start({ kind: 'move', entries: [entry] }) },
          { id: 'cut', label: t('files.menu.cut'), icon: <Scissors aria-hidden />, disabled: why, onSelect: () => useFileClipboard.setState({ environmentId: door.environmentId, mode: 'cut', paths: [entry.path] }) },
          { id: 'copy', label: t('files.menu.copy'), icon: <Copy aria-hidden />, disabled: why, onSelect: () => useFileClipboard.setState({ environmentId: door.environmentId, mode: 'copy', paths: [entry.path] }) },
          { id: 'paste', label: t('files.menu.paste'), icon: <ClipboardPaste aria-hidden />, disabled: pasteBlocked, onSelect: () => paste(entry.path) },
          { id: 'sep-copy', type: 'separator' },
          { id: 'copy-path', label: t('files.menu.copyPath'), icon: <Copy aria-hidden />, onSelect: () => void copyText(entry.path) },
          { id: 'copy-link', label: t('files.menu.copyLink'), icon: <Link2 aria-hidden />, onSelect: () => void copyText(linkTo(entry.path)) },
          { id: 'search-in', label: t('files.menu.searchIn'), icon: <FolderSearch aria-hidden />, onSelect: () => search(entry.path) },
          { id: 'refresh', label: t('files.menu.refresh'), icon: <RefreshCw aria-hidden />, onSelect: () => void refresh(entry.path) },
          { id: 'sep-danger', type: 'separator' },
          { id: 'delete', label: t('files.menu.delete'), icon: <Trash2 aria-hidden />, danger: true, shortcut: 'Delete', disabled: why, onSelect: () => start({ kind: 'delete', entries: [entry] }) },
        );
        return list;
      }
      const openWith = viewersFor(facts).map(
        (viewer): MenuEntry => ({
          id: `with-${viewer.id}`,
          label: t(viewer.title),
          onSelect: () => {
            // The choice is the person's for this kind of file from now on, and the file opens in it.
            rememberChoice(userId, entry.name, viewer.id);
            open(entry, true);
          },
        }),
      );
      list.push(
        { id: 'open', label: t('files.menu.open'), icon: <ExternalLink aria-hidden />, shortcut: 'Enter', onSelect: () => open(entry, false) },
        ...(openWith.length > 1 ? [{ type: 'sub' as const, id: 'open-with', label: t('files.menu.openWith'), entries: openWith }] : []),
        { id: 'download', label: t('files.menu.download'), icon: <Download aria-hidden />, onSelect: () => void downloadFile(door, entry.path, entry.name).catch(() => toast({ tone: 'danger', title: t('files.downloadFailed') })) },
        { id: 'sep-edit', type: 'separator' },
        { id: 'rename', label: t('files.menu.rename'), icon: <Pencil aria-hidden />, shortcut: 'F2', disabled: why, onSelect: () => start({ kind: 'rename', entry }) },
        { id: 'duplicate', label: t('files.menu.duplicate'), icon: <CopyPlus aria-hidden />, disabled: why, onSelect: () => start({ kind: 'duplicate', entry }) },
        { id: 'move', label: t('files.menu.move'), icon: <FolderInput aria-hidden />, disabled: why, onSelect: () => start({ kind: 'move', entries: [entry] }) },
        { id: 'cut', label: t('files.menu.cut'), icon: <Scissors aria-hidden />, disabled: why, onSelect: () => useFileClipboard.setState({ environmentId: door.environmentId, mode: 'cut', paths: [entry.path] }) },
        { id: 'copy', label: t('files.menu.copy'), icon: <Copy aria-hidden />, disabled: why, onSelect: () => useFileClipboard.setState({ environmentId: door.environmentId, mode: 'copy', paths: [entry.path] }) },
        { id: 'sep-copy', type: 'separator' },
        { id: 'copy-path', label: t('files.menu.copyPath'), icon: <Copy aria-hidden />, onSelect: () => void copyText(entry.path) },
        { id: 'copy-relative', label: t('files.menu.copyPathFromDirectory'), icon: <Copy aria-hidden />, onSelect: () => void copyText(fromDirectory(entry.path)) },
        { id: 'copy-reference', label: t('people.reference.copy'), icon: <Link2 aria-hidden />, onSelect: () => void copyText(environmentFileReference(door.environmentId, entry.path), t('people.reference.copied')) },
        { id: 'copy-link', label: t('files.menu.copyLink'), icon: <Link2 aria-hidden />, onSelect: () => void copyText(linkTo(entry.path)) },
        ...(door.threadId === null ? [] : [{ id: 'add', label: t('files.addToMessage'), icon: <MessageSquarePlus aria-hidden />, onSelect: () => { if (door.threadId !== null) addToMessage(door.threadId, { kind: 'file', path: entry.path }); } }]),
        { id: 'sep-danger', type: 'separator' },
        { id: 'delete', label: t('files.menu.delete'), icon: <Trash2 aria-hidden />, danger: true, shortcut: 'Delete', disabled: why, onSelect: () => start({ kind: 'delete', entries: [entry] }) },
      );
      return list;
    },
    // The entries are made when a menu opens; they read the state of that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [t, why, pasteBlocked, door, access.directory, open, paste, refresh, search, start],
  );

  const emptyEntries = useCallback(
    (directory: string): MenuEntry[] => [
      { id: 'new-file', label: t('files.menu.newFile'), icon: <FilePlus aria-hidden />, disabled: why, onSelect: () => start({ kind: 'newFile', directory }) },
      { id: 'new-folder', label: t('files.menu.newFolder'), icon: <FolderPlus aria-hidden />, disabled: why, onSelect: () => start({ kind: 'newFolder', directory }) },
      {
        id: 'upload',
        label: t('files.menu.upload'),
        icon: <Upload aria-hidden />,
        disabled: why,
        onSelect: () => {
          pickInto.current = directory;
          picker.current?.click();
        },
      },
      { id: 'paste', label: t('files.menu.paste'), icon: <ClipboardPaste aria-hidden />, disabled: pasteBlocked, onSelect: () => paste(directory) },
      { id: 'sep', type: 'separator' },
      { id: 'refresh', label: t('files.menu.refresh'), icon: <RefreshCw aria-hidden />, onSelect: () => void refresh(directory) },
      { id: 'collapse', label: t('files.menu.collapseAll'), icon: <ChevronsDownUp aria-hidden />, onSelect: collapseAll },
    ],
    [t, why, pasteBlocked, paste, refresh, collapseAll, start],
  );

  const pick = (directory: string) => {
    pickInto.current = directory;
    picker.current?.click();
  };

  const move = (target: DirEntry, paths: readonly string[]) => {
    if (why !== false) {
      toast({ tone: 'info', title: why });
      return;
    }
    void (async () => {
      try {
        for (const from of paths) {
          if (from === target.path || target.path.startsWith(`${from}/`) || parentOf(from) === target.path) continue;
          await renameFile(door, from, joinPath(target.path, baseName(from)));
        }
        await refresh(target.path, ...paths.map(parentOf));
      } catch {
        toast({ tone: 'danger', title: t('files.menu.moveFailed') });
      }
    })();
  };

  const picking = (
    <input
      ref={picker}
      type="file"
      multiple
      tabIndex={-1}
      aria-label={t('files.menu.upload')}
      className="sr-only"
      onChange={(event) => {
        const files = [...(event.target.files ?? [])];
        event.target.value = '';
        if (files.length > 0) upload(pickInto.current, files);
      }}
    />
  );

  const dialogs = (
    <>{uploads.dialog}<FileActDialogs
      act={act}
      blocked={why}
      onClose={() => setAct(null)}
      door={door}
      environmentName={access.environment.name}
      refresh={refresh}
      open={open}
      list={entries}
    /></>
  );

  return { entriesFor, emptyEntries, start, upload, pick, move, dialogs, picking, progress: uploads.progress };
}

// ---------------------------------------------------------------- the dialogs

interface DialogsProps {
  blocked: string | false;
  act: FileAct | null;
  onClose: () => void;
  door: Door;
  environmentName: string;
  refresh: (...directories: string[]) => Promise<void>;
  open: (entry: DirEntry, permanent: boolean) => void;
  list: () => readonly DirEntry[];
}

function FileActDialogs({ act, blocked, onClose, door, environmentName, refresh, open }: DialogsProps) {
  if (act === null) return null;
  if (act.kind === 'delete') return <DeleteDialog act={act} blocked={blocked} onClose={onClose} door={door} environmentName={environmentName} refresh={refresh} />;
  return <NameDialog key={`${act.kind}.${'entry' in act ? act.entry.path : 'directory' in act ? act.directory : act.entries.map((entry) => entry.path).join(',')}`} act={act} blocked={blocked} onClose={onClose} door={door} refresh={refresh} open={open} />;
}

function DeleteDialog({ act, blocked, onClose, door, environmentName, refresh }: { blocked: string | false; act: Extract<FileAct, { kind: 'delete' }>; onClose: () => void; door: Door; environmentName: string; refresh: DialogsProps['refresh'] }) {
  const { t } = useTranslation();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [first] = act.entries;
  const hasDirectory = act.entries.some((entry) => entry.type === 'dir');
  return (
    <ConfirmDialog
      open
      onOpenChange={(next) => !next && onClose()}
      title={act.entries.length === 1 && first !== undefined ? t('files.delete.title', { name: first.name }) : t('files.delete.titleMany', { count: act.entries.length })}
      confirmLabel={t('files.delete.confirm')}
      danger
      pending={pending}
      error={error}
      disabled={blocked !== false}
      onConfirm={() => {
        if (blocked !== false || pending) return;
        setPending(true);
        setError(null);
        void (async () => {
          try {
            for (const entry of act.entries) await deleteFile(door, entry.path, entry.type === 'dir');
            await refresh(...act.entries.map((entry) => parentOf(entry.path)));
            onClose();
          } catch (failure) {
            setError(failure);
            setPending(false);
          }
        })();
      }}
    >
      {blocked !== false && <p role="alert" className="text-danger">{blocked}</p>}
      <p>{t('files.delete.what', { name: environmentName })}</p>
      {hasDirectory && <p>{t('files.delete.withContents')}</p>}
    </ConfirmDialog>
  );
}

function NameDialog({ act, blocked, onClose, door, refresh, open }: { blocked: string | false; act: Exclude<FileAct, { kind: 'delete' }>; onClose: () => void; door: Door; refresh: DialogsProps['refresh']; open: DialogsProps['open'] }) {
  const { t } = useTranslation();
  const initial =
    act.kind === 'rename'
      ? act.entry.name
      : act.kind === 'duplicate'
        ? copyName(act.entry.name)
        : act.kind === 'move'
          ? (act.entries[0] === undefined ? '' : parentOf(act.entries[0].path))
          : '';
  const [text, setText] = useState(initial);
  const [error, setError] = useState<unknown>(null);
  const [pending, setPending] = useState(false);
  const moving = act.kind === 'move';
  const fault = moving ? null : nameFault(text);
  const [touched, setTouched] = useState(false);

  const title =
    act.kind === 'newFile' ? t('files.name.newFile') : act.kind === 'newFolder' ? t('files.name.newFolder') : act.kind === 'rename' ? t('files.name.rename') : act.kind === 'duplicate' ? t('files.name.duplicate') : t('files.name.move');
  const label = moving ? t('files.name.destination') : t('files.name.name');
  const confirmWord =
    act.kind === 'newFile' || act.kind === 'newFolder'
      ? t('files.name.confirm.create')
      : act.kind === 'rename'
        ? t('files.name.confirm.rename')
        : act.kind === 'duplicate'
          ? t('files.name.confirm.duplicate')
          : t('files.name.confirm.move');

  const submit = () => {
    setTouched(true);
    if (fault !== null || blocked !== false || pending) return;
    setPending(true);
    setError(null);
    void (async () => {
      try {
        const name = text.trim();
        switch (act.kind) {
          case 'newFile': {
            const path = joinPath(act.directory, name);
            await writeFile(door, path, '', 'none');
            await refresh(act.directory);
            onClose();
            open({ ...blank(path, name) }, true);
            return;
          }
          case 'newFolder':
            await makeDirectory(door, joinPath(act.directory, name));
            await refresh(act.directory);
            break;
          case 'rename':
            await renameFile(door, act.entry.path, joinPath(parentOf(act.entry.path), name));
            await refresh(parentOf(act.entry.path));
            break;
          case 'duplicate':
            await copyFile(door, act.entry.path, joinPath(parentOf(act.entry.path), name));
            await refresh(parentOf(act.entry.path));
            break;
          case 'move': {
            const destination = cleanDirectory(text);
            for (const entry of act.entries) await renameFile(door, entry.path, joinPath(destination, entry.name));
            await refresh(destination, ...act.entries.map((entry) => parentOf(entry.path)));
            break;
          }
        }
        onClose();
      } catch (failure) {
        setError(failure);
        setPending(false);
      }
    })();
  };

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !pending && onClose()}
      title={title}
      size="sm"
      dismissible={!pending}
      footer={
        <>
          <Button onClick={onClose} disabled={pending}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={pending} disabled={blocked !== false} onClick={submit}>
            {confirmWord}
          </Button>
        </>
      }
    >
      {blocked !== false && <p role="alert" className="text-danger">{blocked}</p>}
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Input
          label={label}
          mono
          autoFocus
          value={text}
          onChange={(event) => setText(event.target.value)}
          onFocus={(event) => {
            if (act.kind === 'rename') event.target.setSelectionRange(0, stemEnd(event.target.value));
          }}
          error={touched && fault !== null ? t(`files.name.fault.${fault}`) : undefined}
          hint={moving ? t('files.name.destinationHint') : undefined}
        />
        {error !== null && <ErrorAlert error={error} />}
      </form>
    </Dialog>
  );
}

/** The end of a file's name without its extension, so that renaming selects what people change. */
function stemEnd(name: string): number {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? dot : name.length;
}

/** "notes.md" becomes "notes copy.md". */
export function copyName(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? `${name.slice(0, dot)} copy${name.slice(dot)}` : `${name} copy`;
}

/** The entry a file that was just made stands for, until the directory is read again. */
function blank(path: string, name: string): DirEntry {
  return { cached: false, link_target: null, mode: 0o644, mtime: new Date().toISOString(), name, path, size: 0, type: 'file' };
}

