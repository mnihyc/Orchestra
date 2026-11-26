/** The selected directory's shared instructions are an ordinary version-checked workspace file. */
import { useQuery } from '@tanstack/react-query';
import { Suspense, useState, type ComponentProps, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import type { Environment,WorkspaceFileEdit,NewEnvironmentFiles } from '@/generated/api-types';
import { languageOf } from '@/features/files/languages';
import type { CodeViewProps } from '@/ui/CodeView';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { lazyPart } from '@/features/files/lazy';
import { instructionSourceQuery } from './instructionSource';
import { sourceFileEditsQuery } from '@/api/source';
import type { InstructionRecovery as Recovery } from './InstructionRecovery';
import { useInstructionFile } from './instructionFile';
import { environmentWorkspaceAccess, isAway, workspaceBinding, workspaceDenial } from '@/features/files/door';
import { FileSourceContext, workspaceSource } from '@/features/files/source';
import type { ReadOnlyReason } from '@/features/files/viewers';
import type Viewer from '@/features/files/viewers/Code';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

const FILE_NAME = 'AGENTS.md';
const CodeView = lazyPart<CodeViewProps>(() => import('@/ui/CodeView').then((module) => ({ default: module.CodeView })));
const CodeViewer = lazyPart<ComponentProps<typeof Viewer>>(() => import('@/features/files/viewers/Code'));
const InstructionRecovery = lazyPart<ComponentProps<typeof Recovery>>(() => import('./InstructionRecovery').then((module) => ({ default: module.InstructionRecovery })));

function EditorBoundary({ children }: { children: ReactNode }) {
  return <ContentBoundary><Suspense fallback={<SkeletonRows rows={4} />}>{children}</Suspense></ContentBoundary>;
}

/** Before creation, Save changes only the start plan; the source job owns the eventual conditional write. */
export function StagedWorkspaceInstructions({value,onChange,sourceKey,known=null,files,directory=''}:{value:WorkspaceFileEdit|null;onChange:(value:WorkspaceFileEdit)=>void;sourceKey:string;known?:{text:string;hash:string}|null;files?:NewEnvironmentFiles|null;directory?:string}) {
  const {t}=useTranslation();
  const [open,setOpen]=useState(false);
  const source=useQuery({...instructionSourceQuery(files,sourceKey,directory),enabled:open&&files!==undefined&&files!==null});
  const current=source.isError?null:source.data??known;
  const baseline=value?.text??current?.text??'';
  const [edited,setText]=useState<string|null>(null);
  const text=edited??baseline;
  const language=languageOf(FILE_NAME,null);
  const save=()=>onChange({path:FILE_NAME,text,expected_hash:value===null?current?.hash??null:value.expected_hash??null});
  return <details className="min-w-0" onToggle={(event)=>setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm font-medium">{FILE_NAME}{value!==null&&<span className="ms-2 font-normal text-text-muted">{t('start.instructions.edited')}</span>}</summary>
    {open&&<EditorBoundary>{files!==undefined&&files!==null&&source.isPending?<SkeletonRows read={source} rows={4}/>:source.isError?<ErrorState compact error={source.error} onRetry={()=>void source.refetch()}/>:<div className="flex min-w-0 flex-col gap-1">
      <div className="flex justify-end"><Button size="sm" variant="primary" disabled={value!==null&&text===baseline} onClick={save}>{t('files.edit.save')}</Button></div>
      <div className="h-80 min-h-0 overflow-hidden rounded-md border border-border">
        <CodeView text={text} version={sourceKey} label={t('files.code.label',{name:FILE_NAME})} language={language?.load??null} languageKey={language?.id??null} className="h-full" onChange={setText} onSave={save}/>
      </div>
    </div>}</EditorBoundary>}
  </details>;
}

export function WorkspaceInstructions({ environment, environmentId = environment?.id, directory,recovery }: { environment?: Environment; environmentId?: string; directory: string;recovery?:{thread:string;state:string} }) {
  const [open, setOpen] = useState(false);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const details = useQuery({ ...environmentQuery(environmentId ?? ''), enabled: open && environment === undefined && environmentId !== undefined, retry: false });
  const saved=useQuery({...sourceFileEditsQuery(environmentId??'',recovery?.thread??'',JSON.stringify([reader,recovery?.state])),enabled:open&&recovery!==undefined&&environmentId!==undefined});
  const current = environment?.id === environmentId ? environment : !details.isError && details.data?.id === environmentId ? details.data : undefined;
  const root = directory.replace(/\/+$/u, '');
  const path = root === '' ? FILE_NAME : `${root}/${FILE_NAME}`;
  if (environmentId === undefined) return null;
  return <details className="min-w-0" onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm font-medium">{FILE_NAME}</summary>
    {open && <EditorBoundary>{current === undefined ? details.isError ? <ErrorState compact error={details.error} onRetry={() => void details.refetch()} /> : <SkeletonRows read={details} rows={4} />
      :recovery!==undefined&&saved.isPending?<SkeletonRows read={saved} rows={4}/>
      :recovery!==undefined&&saved.isError?<ErrorState compact error={saved.error} onRetry={()=>void saved.refetch()}/>
      :recovery!==undefined&&saved.data?.edits.some((edit)=>edit.path===FILE_NAME)===true?<FileSourceContext.Provider value={workspaceSource({environmentId:current.id,threadId:null},path,FILE_NAME)}><InstructionRecovery key={JSON.stringify([reader,workspaceBinding(current),path,saved.data.revision])} environment={current} path={path} thread={recovery.thread} files={saved.data}/></FileSourceContext.Provider>
      : <InstructionAccess key={JSON.stringify([reader, current.id, workspaceBinding(current), path])} environment={current} path={path} />}</EditorBoundary>}
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
  const {head,error,loading,reload,door,binding,directory,capacity,tree}=useInstructionFile(environment,path);
  const absent = error instanceof ApiError && error.is('not_found') && error.problem.reason === 'not_found' && !isAway(environment);
  const [wasAbsent, setWasAbsent] = useState(false);
  if (absent && !wasAbsent) setWasAbsent(true);
  else if (head !== undefined && error === null && wasAbsent) setWasAbsent(false);
  // Keep the same editor through the first successful read after exclusive creation or a refused save.
  const editingAbsent = absent || wasAbsent && head === undefined && error === null;
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
