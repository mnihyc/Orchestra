/** Access requests on a permission page use the same timed decisions and settled status as the Inbox and reference card. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { waitingRequestsQuery } from '@/api/access';
import { AccessCard, type AccessApproval } from '@/features/approvals/Access';
import type { Approval } from '@/generated/api-types';
import { useSocketStatus } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';

export function RequestCard({approval}:{approval:AccessApproval}) {
  const [settled,setSettled]=useState<ReadonlyMap<string,Approval>>(new Map());
  const live=useSocketStatus().phase==='ready';
  return <li><AccessCard items={[approval]} settled={settled} live={live} onSettled={(next)=>setSettled((before)=>new Map(before).set(next.id,next))} /></li>;
}

/** Share starts with adding recipients; decisions are read when their review is opened. */
export function RequestReview({nodeId}:{nodeId:string}) {
  const {t}=useTranslation();
  const [open,setOpen]=useState(false);
  return <Popover label={t('access.waiting.review')} open={open} onOpenChange={setOpen}
    className="w-(--menu-max-width)" trigger={<Button size="sm" variant="ghost" className="self-start">{t('access.waiting.review')}</Button>}>
    {open && <WaitingRequests nodeId={nodeId} showEmpty />}
  </Popover>;
}

/** The requests that wait on a page, for whoever may decide them. */
export function WaitingRequests({nodeId,showEmpty=false}:{nodeId:string;showEmpty?:boolean}) {
  const {t}=useTranslation();
  const waiting=useQuery(waitingRequestsQuery(nodeId));
  if(waiting.isPending)return <SkeletonRows rows={2} />;
  if(waiting.isError)return <ErrorState compact error={waiting.error} onRetry={()=>void waiting.refetch()} retrying={waiting.isRefetching} />;
  const cards=(waiting.data??[]).filter((entry):entry is AccessApproval=>entry.kind==='access' && entry.can_decide);
  if(cards.length===0)return showEmpty ? <p role="status" className="text-sm text-text-muted">{t('access.waiting.empty')}</p> : null;
  return <section aria-label={t('access.waiting.title')} className="flex flex-col gap-2"><h3 className="text-sm font-medium text-text">{t('access.waiting.title')}</h3><ul className="flex flex-col gap-2">{cards.map((approval)=><RequestCard key={approval.id} approval={approval} />)}</ul></section>;
}
