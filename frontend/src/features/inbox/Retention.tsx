/** The own Inbox boundary is measured independently of the loaded pages. */
import { useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { fetchNotifications } from '@/api/notifications';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';

export function InboxRetention() {
  const {t}=useTranslation();
  const format=useFormatContext();
  const cache=useQueryClient();
  const user=useStore(client.session,(state)=>state.userId);
  const version=useStore(client.session,(state)=>state.readerVersion);
  const key=useMemo(()=>['inbox-retention',user,version] as const,[user,version]);
  const subject=user===null?null:`${user}:${version}`;
  const [opening,setOpening]=useState<string|null>(null);
  const open=subject!==null&&opening===subject;
  const query=useQuery({queryKey:key,queryFn:({signal})=>fetchNotifications({limit:1},signal),enabled:open&&user!==null,retry:false,refetchOnWindowFocus:false,refetchOnReconnect:false});
  useEffect(()=>()=>{void cache.cancelQueries({queryKey:key,exact:true});},[cache,key]);
  const fact=query.isError?undefined:query.data?.retention??undefined;
  const duration=fact===undefined?undefined:formatSpan(fact.days*86_400_000,format,'short');
  return <FactDisclosure open={open} onOpenChange={(next)=>{setOpening(next?subject:null);if(!next)void cache.cancelQueries({queryKey:key,exact:true});}}
    fragment={duration===undefined?undefined:t('placesMail.inboxRetention',{duration})}
    groups={[{key:'inbox',kind:'current_setting',facts:[
      {key:'retention',label:t('placesMail.retention'),value:duration},
      {key:'oldest',label:t('placesMail.oldest'),value:fact===undefined?undefined:fact.actual_oldest_retained===null?t('placesMail.none'):<RelativeTime at={fact.actual_oldest_retained}/>},
      {key:'cutoff',label:t('placesMail.expiry'),value:fact===undefined?undefined:<RelativeTime at={fact.expires_before}/>},
      {key:'source',label:t('placesMail.source'),value:fact===undefined?undefined:<SourceSettingLink source={fact.source} def={{key:'retention.inbox_days',class:'policy'}} target={{kind:'system'}} fallback={t('settings.editor.from.system')}/>},
      {key:'measured',label:t('placesMail.observed'),value:fact===undefined?undefined:<RelativeTime at={fact.measured_at}/>},
    ]}]}/>;
}
