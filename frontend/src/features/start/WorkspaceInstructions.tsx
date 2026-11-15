/** The selected directory's shared instructions are an ordinary version-checked workspace file. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import type { Door } from '@/api/workspace';
import type { Environment,WorkspaceFileEdit } from '@/generated/api-types';
import { languageOf } from '@/features/files/languages';
import { CodeView } from '@/ui/CodeView';
import { invalidateHead, useFileHead } from '@/features/files/content';
import { environmentWorkspaceAccess, isAway, workspaceBinding, workspaceDenial } from '@/features/files/door';
import { FileSourceContext, workspaceSource } from '@/features/files/source';
import { useDirectoryTree } from '@/features/files/useDirectoryTree';
import type { ReadOnlyReason } from '@/features/files/viewers';
import CodeViewer from '@/features/files/viewers/Code';
import { directoryKey } from '@/realtime/stores/environment';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

const FILE_NAME = 'AGENTS.md';

/** Before creation, Save changes only the start plan; the source job owns the eventual conditional write. */
export function StagedWorkspaceInstructions({value,onChange,sourceKey,known=null}:{value:WorkspaceFileEdit|null;onChange:(value:WorkspaceFileEdit)=>void;sourceKey:string;known?:{text:string;hash:string}|null}) {
  const {t}=useTranslation();
  const [open,setOpen]=useState(false);
  const baseline=value?.text??known?.text??'';
  const [text,setText]=useState(baseline);
  const language=languageOf(FILE_NAME,null);
  const save=()=>onChange({path:FILE_NAME,text,expected_hash:value===null?known?.hash??null:value.expected_hash??null});
  return <details className="min-w-0" onToggle={(event)=>setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm font-medium">{FILE_NAME}{value!==null&&<span className="ms-2 font-normal text-text-muted">{t('start.instructions.edited')}</span>}</summary>
    {open&&<div className="flex min-w-0 flex-col gap-1">
      <div className="flex justify-end"><Button size="sm" variant="primary" disabled={value!==null&&text===baseline} onClick={save}>{t('files.edit.save')}</Button></div>
      <div className="h-80 min-h-0 overflow-hidden rounded-md border border-border">
        <CodeView text={text} version={sourceKey} label={t('files.code.label',{name:FILE_NAME})} language={language?.load??null} languageKey={language?.id??null} className="h-full" onChange={setText} onSave={save}/>
      </div>
    </div>}
  </details>;
}

export function WorkspaceInstructions({ environment, environmentId = environment?.id, directory }: { environment?: Environment; environmentId?: string; directory: string }) {
  const [open, setOpen] = useState(false);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const details = useQuery({ ...environmentQuery(environmentId ?? ''), enabled: open && environment === undefined && environmentId !== undefined, retry: false });
  const current = environment?.id === environmentId ? environment : !details.isError && details.data?.id === environmentId ? details.data : undefined;
  const root = directory.replace(/\/+$/u, '');
  const path = root === '' ? FILE_NAME : `${root}/${FILE_NAME}`;
  if (environmentId === undefined) return null;
  return <details className="min-w-0" onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm font-medium">{FILE_NAME}</summary>
    {open && (current === undefined ? details.isError ? <ErrorState compact error={details.error} onRetry={() => void details.refetch()} /> : <SkeletonRows read={details} rows={4} />
      : <InstructionAccess key={JSON.stringify([reader, current.id, workspaceBinding(current), path])} environment={current} path={path} />)}
  </details>;
}

function InstructionAccess({ environment, path }: { environment: Environment; path: string }) {
  const { t } = useTranslation();
  const access = environmentWorkspaceAccess(environment);
  const missing = workspaceDenial(t, 'files.view', environment);
  if (!access.mayView) return <EmptyState compact title={missing} />;
  return <InstructionFile environment={environment} path={path} mayManage={access.mayManage} />;
}

function InstructionFile({ environment, path, mayManage }: { environment: Environment; path: string; mayManage: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const door = useMemo<Door>(() => ({ environmentId: environment.id, threadId: null }), [environment.id]);
  const binding = workspaceBinding(environment);
  const { head, error, loading, reload } = useFileHead(door, path, binding);
  const absent = error instanceof ApiError && error.is('not_found') && error.problem.reason === 'not_found' && !isAway(environment);
  const [wasAbsent, setWasAbsent] = useState(false);
  if (absent && !wasAbsent) setWasAbsent(true);
  else if (head !== undefined && error === null && wasAbsent) setWasAbsent(false);
  // Keep the same editor through the first successful read after exclusive creation or a refused save.
  const editingAbsent = absent || wasAbsent && head === undefined && error === null;
  const parent = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
  const { directories, capacity, tree } = useDirectoryTree(door, parent, binding);
  const directory = directories.get(directoryKey(environment.id, parent));
  const entry = directory?.entries.find((candidate) => candidate.path === path);
  const observed = useRef<{ binding: string; generation: number; stamp: string } | null>(null);
  useEffect(() => {
    if (directory === undefined || directory.page === null && !directory.invalidated) return;
    const stamp = JSON.stringify([entry?.mtime, entry?.size]);
    const before = observed.current;
    if (before?.binding === binding && before.generation === directory.generation && before.stamp === stamp) return;
    observed.current = { binding, generation: directory.generation, stamp };
    if (before !== null && (before.binding !== binding || before.generation !== directory.generation)
      || entry !== undefined && (entry.mtime !== head?.stat.mtime || entry.size !== head.stat.size)) void invalidateHead(cache, door, path, binding);
  }, [binding, directory, entry, head, cache, door, path]);
  if (directory?.watch === 'refused') return <ErrorState compact error={directory.error} />;
  if (!absent && error instanceof ApiError && (error.status === 403 || error.status === 404)) return <ErrorState compact error={error} onRetry={() => void reload()} />;
  if (error !== null && head === undefined && !editingAbsent) return <ErrorState compact error={error} onRetry={() => void reload()} />;
  const currentHead = editingAbsent ? null : head;
  if (!editingAbsent && loading || currentHead === undefined) return <SkeletonRows rows={4} />;
  const reason: ReadOnlyReason | null = !mayManage ? { kind: 'permission' }
    : isAway(environment) ? { kind: 'offline', environment: environment.name, asOf: directory?.asOf ?? null }
    : !environment.remote_write ? { kind: 'no_writes', machine: environment.machine?.name ?? null } : null;
  const paused = capacity !== null || directory?.capacity !== undefined && directory.capacity !== null
    || directory?.watch === 'failed' || directory?.invalidated === true;
  return <div className="flex min-w-0 flex-col gap-1">
    {paused && <div role="status" className="flex items-center gap-1 text-sm text-text-muted"><span>{t('files.tab.updatesPaused')}</span>
      <Button size="sm" variant="ghost" onClick={() => { tree.refresh(); void reload(); }}>{t('common.tryAgain')}</Button></div>}
    <div className="h-80 min-h-0 overflow-hidden rounded-md border border-border">
      <FileSourceContext.Provider value={workspaceSource(door, path, FILE_NAME)}>
        <CodeViewer door={door} path={path} head={currentHead} workspaceBinding={binding} line={null} whyReadOnly={reason} thread={null} openAs={() => undefined} />
      </FileSourceContext.Provider>
    </div>
  </div>;
}
