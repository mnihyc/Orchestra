/** Slack-style notification exceptions, with the node bell's authority and inheritance. */
import { useMutation,useQuery,useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { notificationExceptionsQuery,setNotificationException } from '@/api/notificationExceptions';
import { PlaceLink } from '@/features/navigation/places';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { destinationOf, pathOf } from '@/features/finder/destinations';
import { useOpenPath } from '@/features/chat/open';
import { NamePicker } from '@/features/search/NamePicker';
import { acceptSubscription } from '@/features/notifications/subscription';
import type { NotificationException,NotificationExceptionChanged,SubscriptionLevel } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorAlert,ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { linkClasses } from '@/ui/link';

const LEVELS=['all','important','off'] as const;
interface Change {row:NotificationException;level:SubscriptionLevel|null}
interface Previous {key:readonly ['subscription-choice',string];before:SubscriptionLevel|null|undefined}
export function NotificationExceptions() {
  const {t}=useTranslation();const cache=useQueryClient();
  const openPath=useOpenPath();
  const [cursors,setCursors]=useState<(string|null)[]>([null]);
  const cursor=cursors.at(-1) ?? null;
  const query=useQuery(notificationExceptionsQuery(cursor));
  const labels=usePlaceLabels((query.data?.items ?? []).map((row)=>({id:row.node.id,name:row.node.name ?? t('access.place.hidden'),
    above:row.path.map((part)=>({id:part.id,name:part.redacted?null:part.name}))})),{trees:false});
  const nextCursor=query.data?.next_cursor ?? null;
  const save=useMutation<NotificationExceptionChanged,Error,Change,Previous>({mutationKey:['notification-exception'],scope:{id:'notification-subscriptions'},
    mutationFn:({row,level}:Change)=>setNotificationException(row.node.id,level,cursor),
    onMutate:({row,level})=>{
      const key=['subscription-choice',row.node.id] as const;
      const before=cache.getQueryData<SubscriptionLevel|null>(key);cache.setQueryData(key,level);
      return {key,before};
    },
    onError:(_error,change,context)=>{
      if(context!==undefined && cache.getQueryData(context.key)===change.level){if(context.before===undefined)cache.removeQueries({queryKey:context.key,exact:true});else cache.setQueryData(context.key,context.before);}
      // A refusal may be a lost View or a simultaneous choice on another device. Reconcile the
      // visible page; a failed write supplies no authoritative replacement snapshot.
      void cache.invalidateQueries({queryKey:notificationExceptionsQuery(cursor).queryKey,exact:true});
    },
    onSuccess:async(reply,{row})=>{
      await acceptSubscription(cache,row.node.id,reply,cursor);
      toast({tone:'success',title:t('notificationExceptions.saved'),action:{label:t('settings.compact.undo'),run:()=>save.mutate({row,level:row.level})}});
    },
  });
  return <Section id="notification-exceptions" title={t('notificationExceptions.title')}>
    {query.isPending?<SkeletonRows rows={2}/>:query.isError?<ErrorState compact error={query.error} onRetry={()=>void query.refetch()} />:<>
      {query.data.items.length===0?<p className="text-sm text-text-muted">{t(query.data.next_cursor!==null?'notificationExceptions.morePrivate':cursors.length>1?'notificationExceptions.pageEmpty':'notificationExceptions.empty')}</p>:
        <ul aria-label={t('notificationExceptions.title')} className="divide-y divide-border">
          {query.data.items.map((row)=>{
            const name=labels.get(row.node.id) ?? row.node.name ?? t('access.place.hidden');
            const source=row.default_from;
            const inherited=source===null?t('notificationExceptions.systemDefault'):t('notificationExceptions.inherited',{place:source.redacted?t('access.place.hidden'):source.name ?? t('access.place.hidden')});
            return <li key={row.node.id} className="flex min-w-0 flex-wrap items-start justify-between gap-x-3 gap-y-2 py-2">
              <div className="min-w-0 flex-1 basis-40 text-sm leading-(--control-sm) wrap-anywhere"><PlaceLink place={row.node} className={linkClasses}>{name}</PlaceLink></div>
              <Select<SubscriptionLevel|'default'> size="sm" hideLabel label={t('notificationExceptions.level',{place:name})} value={row.level} disabled={save.isPending}
                onValueChange={(level)=>save.mutate({row,level:level==='default'?null:level})} fieldClassName="w-full sm:w-(--field-wide)"
                options={[{value:'default',label:t('notificationExceptions.default',{level:t(`notifications.levels.${row.default_level}.name`)}),description:inherited},
                  ...LEVELS.map((level)=>({value:level,label:t(`notifications.levels.${level}.name`),description:t(`notifications.levels.${level}.what`)}))]}/>
            </li>;
          })}
        </ul>}
      <NamePicker label={t('sharedWithMe.findPlaces')} kinds={['org','project','topic','thread']} chosen={null}
        placeholder={t('settings.place.placeholder')} nothing={t('settings.place.none')}
        onChoose={(hit)=>{const destination=hit===null?null:destinationOf(hit);if(destination!==null)openPath(pathOf(destination));}} />
      {(cursors.length>1 || nextCursor!==null) && <div className="flex items-center gap-2">
        {cursors.length>1 && <Button size="sm" disabled={save.isPending} onClick={()=>setCursors((before)=>before.slice(0,-1))}>{t('common.back')}</Button>}
        {nextCursor!==null && <Button size="sm" disabled={save.isPending} onClick={()=>setCursors((before)=>[...before,nextCursor])}>{t('common.next')}</Button>}
      </div>}
    </>}
    {save.isError && <ErrorAlert error={save.error}/>}
  </Section>;
}
