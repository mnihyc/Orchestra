/** One accepted session revision, carried through the thread reader's existing scope and cancellation. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { threadQueryRead, readInThread, threadReadSelector, receiveThreadRead } from '@/api/threadRead';
import { stampThreadReceipt, type ThreadReadTarget } from '@/realtime/threadScope';
import type { SessionSkillsSummary, ThreadSessionSkills } from '@/generated/api-types';

export function sessionSkillsQuery(target: ThreadReadTarget, summary: SessionSkillsSummary) {
  const threadId=typeof target==='string'?target:target.threadId;
  const reading=threadQueryRead(target,['thread-session-skills',threadId,summary.segment_id,summary.session_revision]);
  return infiniteQueryOptions({
    queryKey:reading.key,enabled:reading.enabled,initialPageParam:null as string|null,
    queryFn:async({pageParam,signal}):Promise<ThreadSessionSkills>=>{
      const captured=await reading.capture(signal,pageParam!==null);
      const page=await readInThread(captured,(readSignal)=>ok(api.GET('/threads/{id}/session-skills',{
        params:{path:{id:captured.ref.threadId},query:{segment_id:summary.segment_id,session_revision:summary.session_revision,
          ...(pageParam===null?{}:{cursor:pageParam}),...threadReadSelector(captured)}},signal:readSignal,
      })),signal);
      return stampThreadReceipt(page,receiveThreadRead(captured,page.projection,signal));
    },
    getNextPageParam:(page)=>page.entries.next_cursor??undefined,
  });
}
