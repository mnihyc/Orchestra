/** A refused start keeps the proposal beside the actual shared file until the person resolves it. */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { prepareSource } from '@/api/source';
import type { Environment,WorkspaceFileEdits } from '@/generated/api-types';
import { useFileText } from '@/features/files/content';
import { ChangedBar } from '@/features/files/Editing';
import { isAway } from '@/features/files/door';
import { useInstructionFile } from './instructionFile';
import { languageOf } from '@/features/files/languages';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { CodeView } from '@/ui/CodeView';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function InstructionRecovery({environment,thread,path,files}:{environment:Environment;thread:string;path:string;files:WorkspaceFileEdits}) {
  const {t}=useTranslation();
  const actKey=useActKey();
  const found=files.edits.find((edit)=>edit.path==='AGENTS.md');
  const [text,setText]=useState(found?.text??'');
  const [base,setBase]=useState(found?.expected_hash??null);
  const [comparing,setComparing]=useState(false);
  const {head,error,loading,reload,door,directory,capacity,tree}=useInstructionFile(environment,path);
  const loaded=useFileText(door,path,head?.brought);
  const missing=error instanceof ApiError&&error.is('not_found')&&error.problem.reason==='not_found'&&!isAway(environment);
  const current=missing?{text:'',hash:null}:loaded.text;
  const conflict=current!==null&&current.hash!==base&&current.text!==text;
  const manageable=holds(environment.access,'environment.manage')&&!isAway(environment)&&environment.remote_write;
  const language=languageOf('AGENTS.md',null);
  const save=useMutation({mutationFn:()=>{
    const next={revision:files.revision,edits:files.edits.map((edit)=>edit.path==='AGENTS.md'?{...edit,text,expected_hash:base}:edit)};
    return prepareSource(environment.id,thread,actKey(next),undefined,next);
  }});
  if(found===undefined)return null;
  if(directory?.watch==='refused')return <ErrorState compact error={directory.error}/>;
  if(error!==null&&!missing)return <ErrorState compact error={error} onRetry={()=>void reload()}/>;
  if(loaded.error!==null)return <ErrorState compact error={loaded.error} onRetry={()=>void reload()}/>;
  if(loaded.text?.cut===true)return <EmptyState compact title={t('fileMedia.tooLarge')}/>;
  if(!missing&&(loading||loaded.loading)||current===null)return <SkeletonRows rows={4}/>;
  const paused=capacity!==null||directory?.capacity!==undefined&&directory.capacity!==null||directory?.watch==='failed'||directory?.invalidated===true;
  return <div className="flex min-w-0 flex-col gap-1">
    {paused&&<div role="status" className="flex items-center gap-1 text-sm text-text-muted"><span>{t('files.tab.updatesPaused')}</span><Button size="sm" variant="ghost" onClick={()=>{tree.refresh();void reload();}}>{t('common.tryAgain')}</Button></div>}
    {conflict&&<ChangedBar by={null} comparing={comparing} pending={save.isPending} onCompare={()=>setComparing(true)} onKeepMine={()=>setBase(current.hash)} onTakeTheirs={()=>{setText(current.text);setBase(current.hash);}}/>}
    <div className="flex justify-end"><Button size="sm" variant="primary" pending={save.isPending} blocked={!manageable||conflict} reason={!manageable?t('resources.act.manageRequired'):conflict?t('files.edit.changed.title'):undefined} onClick={()=>{if(manageable&&!conflict)save.mutate();}}>{t('files.edit.save')}</Button></div>
    {save.error!==null&&<ErrorState compact error={save.error}/>}
    <div className={comparing?'grid h-96 min-h-0 grid-cols-1 grid-rows-2 gap-px overflow-hidden rounded-md border border-border @3xl:grid-cols-2 @3xl:grid-rows-1':'h-80 min-h-0 overflow-hidden rounded-md border border-border'}>
      <CodeView className="h-full min-h-0" text={text} version={files.revision} label={t('files.code.label',{name:'AGENTS.md'})} language={language?.load??null} languageKey={language?.id??null} readOnly={!manageable} onChange={setText} onSave={()=>{if(manageable&&!conflict)save.mutate();}}/>
      {comparing&&<CodeView className="h-full min-h-0" text={current.text} version={current.hash??'absent'} label={t('files.edit.changed.theirsLabel',{name:'AGENTS.md'})} language={language?.load??null} languageKey={language?.id??null} readOnly/>}
    </div>
  </div>;
}
