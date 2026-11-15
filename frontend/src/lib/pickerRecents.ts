/** Shared recent ordering uses only choices admitted by the current native picker. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import { pickerRecentsKey, pickerRecentsQuery, recordPickerChoice } from '@/api/pickerRecents';
import type { MePickerRecentChoice } from '@/generated/api-types';

export function usePickerRecents(open: boolean) {
  const person=useStore(client.session,(state)=>state.userId);
  const reader=useStore(client.session,(state)=>state.readerVersion);
  const cache=useQueryClient();
  const recent=useQuery({...pickerRecentsQuery(person??''),enabled:open&&person!==null});
  const save=useMutation({
    mutationFn:(choice:MePickerRecentChoice)=>{
      const current=client.session.getState();
      if(current.userId!==person||current.readerVersion!==reader)throw new NetworkError('aborted');
      return recordPickerChoice(choice);
    },
    scope:{id:`picker-recents.${person??''}`},
    onSuccess:(answer)=>{
      const current=client.session.getState();
      if(person!==null&&current.userId===person&&current.readerVersion===reader) cache.setQueryData(pickerRecentsKey(person),answer);
    },
  });
  return {
    loading:person!==null&&recent.isPending,
    error:recent.error,
    retry:()=>{void recent.refetch();},
    choices:person===null?undefined:recent.data?.choices,
    count:person===null?0:recent.data?.count??0,
    record:(choice:MePickerRecentChoice)=>{if(person!==null&&recent.data?.count!==0)save.mutate(choice);},
  };
}

/** Keeps the source order beneath Recent, removes duplicates and leaves disabled entries with their reasons. */
export function recentChoiceRows<T>(rows:readonly T[],keys:readonly string[],key:(row:T)=>string|undefined,enabled:(row:T)=>boolean):T[] {
  const offered=new Map(rows.filter(enabled).flatMap((row)=>{const value=key(row);return value===undefined?[]:[[value,row] as const];}));
  return [...new Set(keys)].flatMap((value)=>{const row=offered.get(value);return row===undefined?[]:[row];});
}
