/** Native addresses and one-level completion share the ordinary paged Files reader. */
import { useQuery } from '@tanstack/react-query';
import { ArrowRight, Folder, File } from 'lucide-react';
import { useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { listDirectory, statFile, type Door } from '@/api/workspace';
import type { Environment } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { Combobox } from '@/ui/Combobox';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { IconButton } from '@/ui/IconButton';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

export const normalizedPath = (path: string): string => path.replaceAll('\\', '/');
export const absolutePath = (path: string): boolean => path.startsWith('/') || /^[a-z]:\//i.test(path);
const rootPath = (path: string): string => /^[a-z]:\//i.test(path) ? path.slice(0, 3)
  : path.startsWith('//') ? path.split('/').slice(0, 4).join('/') + '/' : '/';
const trimmedPath = (path: string): string => path === rootPath(path) ? path : path.replace(/\/+$/, '');
const enteredPath = (path: string): string => path === '' || path.endsWith('/') ? path : path + '/';
const below = (path: string, root: string): boolean => path === root || path.startsWith(root.replace(/\/+$/, '') + '/');

interface Props {
  door: Door;
  environment: Pick<Environment, 'workspace_root' | 'mounts' | 'file_reach'>;
  path: string;
  disabled?: boolean;
  navigate: (path: string) => void;
  openFile: (path: string) => void;
  refuse: (path: string, error: unknown) => void;
}

export function PathBar({ door, environment, path, disabled = false, navigate, openFile, refuse }: Props) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state) => state.readerVersion);
  const [text, setText] = useState(() => enteredPath(normalizedPath(path)));
  const [shown, setShown] = useState(false), [busy, setBusy] = useState(false);
  const delayed = useDebounced(text);
  const [continuation, setContinuation] = useState({ selection: '', cursor: '' });
  const mounted = useRef(false), request = useRef<AbortController | null>(null);
  const identity = JSON.stringify([reader, door.environmentId, door.threadId, path, disabled]);
  const current = useRef(identity);
  useLayoutEffect(() => { current.current = identity; }, [identity]);
  useLayoutEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current?.abort(); }; }, []);
  const slash = delayed.lastIndexOf('/');
  const parent = slash < 0 ? '' : slash === 0 ? '/' : slash === 2 && delayed[1] === ':' ? delayed.slice(0, 3) : delayed.slice(0, slash);
  const prefix = slash < 0 ? delayed : delayed.slice(slash + 1);
  const selection = JSON.stringify([reader, door.environmentId, door.threadId, parent, prefix]);
  const cursor = continuation.selection === selection ? continuation.cursor : '';
  const completion = useQuery({ queryKey: ['files-path-completion', selection, cursor],
    queryFn: ({ signal }) => listDirectory(door, parent, { prefix, ...(cursor === '' ? {} : { cursor }), signal }),
    enabled: shown && !disabled && delayed === text, gcTime: 0, staleTime: 0, retry: false,
    refetchOnWindowFocus: false, refetchOnReconnect: false, meta: { fileEnvironment: door.environmentId } });
  const rows = shown && delayed === text && !completion.isFetching ? completion.data?.entries ?? [] : [];
  const move = (target: string) => {
    if (disabled) return;
    request.current?.abort();
    const controller = new AbortController(), original = identity, originalReader = reader;
    request.current = controller; setBusy(true); setShown(false);
    void statFile(door, target, controller.signal).then((stat) => {
      if (!mounted.current || controller.signal.aborted || current.current !== original || client.session.getState().readerVersion !== originalReader) return;
      if (stat.type === 'dir') navigate(target); else openFile(target);
    }).catch((error: unknown) => {
      if (mounted.current && !controller.signal.aborted && current.current === original && client.session.getState().readerVersion === originalReader) refuse(target, error);
    }).finally(() => { if (mounted.current && request.current === controller) setBusy(false); });
  };
  const mounts = environment.mounts.map((mount) => ({ mount, path: mount.path === null ? null : normalizedPath(mount.path) }));
  const workspace = normalizedPath(environment.workspace_root);
  const actual = normalizedPath(path);
  const selectedMount = mounts.find((mount) => mount.path !== null && below(actual, mount.path));
  const root = selectedMount === undefined ? !absolutePath(actual) || below(actual, workspace) ? 'workspace' : 'machine' : selectedMount.mount.mount.id;
  const machine = environment.file_reach?.view === 'machine';
  const options = [{ value: 'workspace', name: t('fileFacts.workspace') },
    ...mounts.map(({ mount, path: native }) => ({ value: mount.mount.id, name: mount.mount.name, disabled: native === null || mount.state !== 'attached', description: native === null || mount.state !== 'attached' ? t('files.paths.unavailable') : undefined })),
    { value: 'machine', name: t('fileReach.machine'), disabled: !machine, description: machine ? undefined : environment.file_reach?.machine_bound === 'roots' ? t('fileReach.bound', { reach: t('fileReach.roots') }) : t('files.paths.machineRequired') }];
  const confirm = () => move(trimmedPath(normalizedPath(text)));
  return <div className="flex min-w-0 items-center gap-2 border-b border-border px-2 py-1" data-file-path-bar>
    <ResourcePicker pickerKey="files.roots" size="sm" label={t('files.paths.roots')} hideLabel value={root} options={options} disabled={disabled || busy}
      fieldClassName="w-28 shrink-0" onValueChange={(value) => {
        if (disabled) return;
        if (value === 'workspace') navigate('');
        else if (value === 'machine' && machine) navigate(rootPath(absolutePath(actual) ? actual : workspace));
        else { const mount = mounts.find((mount) => mount.mount.mount.id === value); if (mount?.path !== null && mount?.path !== undefined) navigate(absolutePath(mount.path) ? mount.path : workspace.replace(/\/+$/, '') + '/' + mount.path); }
      }} />
    <div className="min-w-0 flex-1" onKeyDown={(event) => {
      if (event.key === 'Enter' && !event.defaultPrevented && !event.nativeEvent.isComposing) { event.preventDefault(); event.stopPropagation(); confirm(); }
    }}>
      <Combobox label={t('files.paths.path')} hideLabel size="sm" query={text} displayValue={text} open={shown}
        onOpenChange={setShown} disabled={disabled} loading={completion.isFetching && shown}
        onQueryChange={(value) => { request.current?.abort(); setBusy(false); setText(normalizedPath(value)); setContinuation({ selection: '', cursor: '' }); }}
        onConfirmQuery={confirm}
        groups={[{ id: 'children', label: '', options: rows.map((entry) => ({ value: entry.path, label: entry.name, text: entry.name,
          icon: entry.type === 'dir' ? <Folder aria-hidden /> : <File aria-hidden /> })) }]}
        onSelect={(option) => { const row = rows.find((row) => row.path === option.value); if (row?.type === 'dir') navigate(option.value); else if (row !== undefined) openFile(option.value); }}
        trailing={<IconButton size="sm" label={t('files.paths.open')} icon={<ArrowRight aria-hidden />} pending={busy} disabled={disabled} onClick={confirm} />}
        empty={null}
        footer={completion.error !== null ? <ErrorAlert error={completion.error} /> : completion.data?.next_cursor !== null && completion.data?.next_cursor !== undefined
          ? <Button size="sm" variant="ghost" onClick={() => setContinuation({ selection, cursor: completion.data?.next_cursor ?? '' })}>{t('files.pages.next')}</Button> : undefined} />
    </div>
  </div>;
}
