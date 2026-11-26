/** A changed opening order starts another read without changing the person's committed choice. */
import {useEffect} from 'react';
import {useQueryClient,type QueryKey} from '@tanstack/react-query';
import {ApiError} from '@/api/client';

export function usePickerCursorRestart({error,isFetchNextPageError}:{error:unknown;isFetchNextPageError:boolean},queryKey:QueryKey):void {
  const cache=useQueryClient();
  useEffect(()=>{
    if(!isFetchNextPageError||!(error instanceof ApiError)||error.code!=='request.invalid'||!error.faults().some((fault)=>fault.path==='cursor'))return;
    // An older observer cannot reset a replacement reader's query with the same key.
    const held=cache.getQueryCache().find({queryKey,exact:true});
    if(held?.state.error!==error)return;
    void cache.resetQueries({queryKey,exact:true});
  },[cache,error,isFetchNextPageError,queryKey]);
}
