/** Bounded personal exception pages; each change returns its replacement page and the ordinary node Access. */
import { queryOptions } from '@tanstack/react-query';
import { api,ok } from '@/api/client';
import type { NotificationExceptionChanged,NotificationExceptionPage,SubscriptionLevel } from '@/generated/api-types';

export const notificationExceptionsKey=['notification-exceptions'] as const;
/** Match the ordinary API page size; settings loads one bounded page rather than every subscription. */
const EXCEPTION_PAGE_SIZE=50;
export const notificationExceptionsQuery=(cursor:string|null)=>queryOptions({
  queryKey:[...notificationExceptionsKey,cursor] as const,
  queryFn:({signal}):Promise<NotificationExceptionPage>=>ok(api.GET('/me/notification-exceptions',{
    params:{query:{limit:EXCEPTION_PAGE_SIZE,...(cursor===null?{}:{cursor})}},signal,
  })),
});
export function setNotificationException(id:string,level:SubscriptionLevel|null,cursor:string|null):Promise<NotificationExceptionChanged> {
  return ok(api.PUT('/me/notification-exceptions/{id}',{params:{path:{id},query:{limit:EXCEPTION_PAGE_SIZE,...(cursor===null?{}:{cursor})}},body:{level}}));
}
