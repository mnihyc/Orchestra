/** The built-in program's same conversation copy, reviewed with its native original ending. */
import {useQuery} from '@tanstack/react-query';
import {useNavigate} from '@tanstack/react-router';
import {useRef,useState} from 'react';
import {useTranslation} from 'react-i18next';
import {useStore} from 'zustand';
import {ApiError,client} from '@/api/client';
import {askReader,moveAsk,previewMoveAsk} from '@/api/ask';
import type {CopyDestinationJudgement} from '@/api/copyDestinations';
import {CopyPlacePicker} from '@/features/duplicate/CopyPlacePicker';
import {NamedChip} from '@/features/people/NamedChip';
import type {PickedPlace} from '@/features/settings/PlacePicker';
import {threadCapabilities} from '@/features/thread/cached';
import type {AskMoveOutBody,Thread} from '@/generated/api-types';
import {newActKey} from '@/lib/requests';
import {Button} from '@/ui/Button';
import {Dialog} from '@/ui/Dialog';
import {ErrorAlert,ErrorState} from '@/ui/ErrorState';
import {Input} from '@/ui/Input';
import {SkeletonRows} from '@/ui/Skeleton';

export function MoveProgram({thread,initialDestination,open,onClose,sourceError=null}:{
  thread:Thread;
  initialDestination?:PickedPlace | undefined;
  open:boolean;
  onClose:()=>void;
  sourceError?:unknown;
}){
  const {t}=useTranslation();
  const navigate=useNavigate();
  const reader=useStore(client.session,(state)=>JSON.stringify([state.userId,state.readerVersion,state.status]));
  const [firstReader]=useState(reader);
  const [place,setPlace]=useState<PickedPlace|null>(initialDestination ?? null);
  const [standing,setStanding]=useState<CopyDestinationJudgement|null>(null);
  const [pending,setPending]=useState(false),[failure,setFailure]=useState<unknown>(null);
  const [attempt,setAttempt]=useState<{body:AskMoveOutBody;key:string}|null>(null);
  const [sideSearch,setSideSearch]=useState('');
  const busy=useRef(false);
  const sameReader=reader===firstReader;
  const working=thread.state==='running'||thread.state==='starting';
  const allowed=thread.program?.id==='orch'&&threadCapabilities(thread)?.fork===true&&!thread.archived&&thread.lock===null;
  const destinationRoot=place?.rootOrgId;
  const destinationReady=destinationRoot!==undefined&&destinationRoot!==thread.program?.root_org_id&&standing!==null&&standing.outcome.state!=='blocked';
  const preview=useQuery({queryKey:['ask-move-preview',thread.id,place?.id,thread.seq,reader],
    queryFn:({signal})=>previewMoveAsk(thread.id,{topic_id:place?.id??null,project_id:null,title:null},signal),
    enabled:open&&sameReader&&allowed&&destinationReady&&sourceError===null&&!pending&&!working,
    staleTime:Infinity,gcTime:0,retry:false,refetchOnWindowFocus:false,refetchOnReconnect:false});
  const plan=preview.data;
  const retryingReceipt=attempt!==null&&failure!==null;
  const ready=sameReader&&!pending&&(retryingReceipt||sourceError===null&&allowed&&destinationReady&&!working&&plan!==undefined&&!preview.isFetching&&plan.open_side_threads.length===0);
  const move=async()=>{
    if(busy.current||!ready)return;
    const wanted=retryingReceipt?attempt:plan===undefined?null:{body:{preflight_id:plan.preflight_id},key:newActKey()};
    if(wanted===null)return;
    busy.current=true;setPending(true);setFailure(null);setAttempt(wanted);
    const current=askReader();
    try{
      const receipt=await moveAsk(thread.id,wanted.body,wanted.key);
      current();onClose();void navigate({to:'/thread/$threadId',params:{threadId:receipt.thread_id}});
    }catch(error){
      try{current();}catch{return;}
      setFailure(error);
      if(error instanceof ApiError&&error.code==='conflict.stale_revision'){
        setAttempt(null);void preview.refetch();
      }
    }finally{busy.current=false;setPending(false);}
  };
  if(!sameReader||!open)return null;
  const wanted=sideSearch.trim().toLocaleLowerCase();
  return <Dialog open title={sourceError===null?t('thread.moveOut.title',{name:thread.name}):t('thread.moveOut.confirm')}
    dismissible={!pending} onOpenChange={(value)=>{if(!value&&!pending)onClose();}}
    footer={<><Button onClick={onClose} disabled={pending}>{t('common.cancel')}</Button><Button variant="primary" pending={pending} disabled={!ready} reason={working?t('thread.now.working'):undefined} onClick={()=>void move()}>{t(retryingReceipt?'common.tryAgain':'thread.moveOut.confirm')}</Button></>}>
    <div className="flex min-w-0 flex-col gap-3">
      {sourceError!==null?<ErrorState compact error={sourceError}/>:<>
        <CopyPlacePicker subject={{kind:'thread',id:thread.id}} intent="move" only="topic" value={place}
          label={t('ask.destination')} disabled={pending} onStanding={setStanding}
          onChange={(next)=>{setPlace(next);setAttempt(null);setFailure(null);}}/>
        {place!==null&&<div aria-label={t('ask.copyRoot')} className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted">
          <span>{t('ask.copyRoot')}</span>{place.rootOrgId===undefined?<span>{t('ask.destinationRootUnknown')}</span>:<NamedChip kind="org" id={place.rootOrgId} path={`/org/${place.rootOrgId}`} {...(place.rootName===undefined?{}:{name:place.rootName})}/>}
        </div>}
        <p className="text-sm text-text-muted">{t('thread.moveOut.about')}</p>
        <span className="text-sm text-text-muted">{t('ask.programDefaults')}</span>
        {preview.isError?<ErrorState compact error={preview.error} onRetry={()=>void preview.refetch()}/>:destinationReady&&plan===undefined?<SkeletonRows read={preview} rows={2}/>:plan!==undefined&&<>
          <ul aria-label={t('duplicate.deletePlan')} className="flex flex-col gap-1 text-sm text-text-muted">{plan.delete_plan.effects.map((effect)=><li key={effect}>{t(`duplicate.moveEffects.${effect}`)}</li>)}</ul>
          {plan.open_side_threads.length>0&&<div className="flex min-w-0 flex-col gap-2">
            <Input label={t('thread.moveOut.searchSides')} value={sideSearch} onChange={(event)=>setSideSearch(event.target.value)}/>
            <ul aria-label={t('thread.moveOut.openSides')} className="max-h-48 overflow-y-auto text-sm">{plan.open_side_threads.filter((side)=>wanted===''||(side.name??'').toLocaleLowerCase().includes(wanted)).map((side)=><li key={side.id}>{side.name??t('side.lane.unnamed')}</li>)}</ul>
          </div>}
        </>}
      </>}
      {failure!==null&&<ErrorAlert error={failure}/>}
    </div>
  </Dialog>;
}
