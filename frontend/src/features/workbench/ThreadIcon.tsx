/** Visible tab icons observe native metadata independently of their paused conversation readers. */
import { useEffect,useMemo, type ComponentProps } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { ApiError,client } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { usePlacePages } from '@/features/navigation/usePlacePages';
import { currentThreadDisplay } from '@/lib/threadDisplay';
import { socket } from '@/realtime/live';
import { useThreadOpeningPaint } from '@/shell/useThreadOpeningPaint';
import { useThreadPart } from '@/realtime/stores/thread';
import { useTreeNode } from '@/realtime/stores/tree';
import { ThreadStateIcon,useThreadStateLabel } from '@/ui/ThreadState';

type Props=ComponentProps<typeof ThreadStateIcon>&{threadId?:string|undefined;rootId?:string|undefined};

export function LiveThreadStateIcon({display,threadId,rootId,className,glyphRef}:Props) {
  const {t}=useTranslation();
  const signedIn=useStore(client.session,(state)=>state.status==='signed_in');
  const painted = useThreadOpeningPaint();
  const phase=useStore(socket.status,(state)=>state.phase);
  const bootstrap=useQuery({...nodeQuery(threadId??''),enabled:painted&&signedIn&&threadId!==undefined&&rootId===undefined,retry:false,gcTime:0});
  const root=bootstrap.isFetchedAfterMount?bootstrap.data?.org_id??rootId:rootId??bootstrap.data?.org_id;
  const roots=useMemo(()=>root===undefined?[]:[{id:root,name:''}],[root]);
  const selection=useMemo(()=>({path_to:threadId??''}),[threadId]);
  const metadata=usePlacePages(roots,selection,painted&&signedIn&&root!==undefined&&threadId!==undefined);
  const {refetch}=bootstrap;
  useEffect(()=>{
    if(signedIn&&rootId!==undefined&&metadata.error instanceof ApiError&&metadata.error.is('not_found'))void refetch();
  },[signedIn,rootId,metadata.error,refetch]);
  const page=metadata.pages[0],native=page?.result.data?.nodes.find((node)=>node.id===threadId)?.thread?.display;
  const row=useTreeNode(threadId);
  const held=useThreadPart(threadId??'',(view)=>view.closed===null?view.thread?.display:null);
  const observed=native===undefined||metadata.error!==null?null:currentThreadDisplay(held??undefined,currentThreadDisplay(native,currentThreadDisplay(row?.thread?.display,display??undefined)))??null;
  const label=useThreadStateLabel(observed);
  const snapshot=observed!==null&&(page?.live!=='current'||phase!=='ready');
  return <ThreadStateIcon display={observed} label={snapshot?t('threadStates.lastReported',{state:label}):undefined} className={className} glyphRef={glyphRef}/>;
}
