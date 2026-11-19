/** Live checklist, and the plan a planning turn proposes.
 *
 * The agent's plan as a card with a checklist. It updates in place as the agent works through it: a
 * step is waiting, under way or done, said in words as well as by its mark. The plan a turn in plan mode
 * ends with is read as the agent wrote it, and while it is the thread's last word it carries
 * Proceed and Keep planning, which answer it through the agent program's declared mechanism.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Check, Circle, LoaderCircle } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { settingsQuery } from '@/api/settings';
import { decidePlan } from '@/api/threads';
import { sourceDecisionOf } from '@/api/approvals';
import type { Item, RequestDecisionStatus, ThreadPlanDecideBody } from '@/generated/api-types';
import { PlanAnswer } from '@/features/approvals/PlanAnswer';
import { DecisionStatus } from '@/features/approvals/Card';
import { composerWriter } from '@/features/composer/Draft';
import { Body } from '@/features/chat/Body';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { holds } from '@/lib/permissions';
import { threadReadState, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useSocketStatus } from '@/realtime/live';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';

type PlanItem = Extract<Item, { kind: 'plan' }>;

export function Plan({ item }: { item: PlanItem }) {
  const { t } = useTranslation();
  const { steps, explanation } = item.payload;
  const done = steps.filter((step) => step.status === 'completed').length;
  const proposed = useThreadPart(item.thread_id, (view) => item.turn_id !== null && view.turns.get(item.turn_id)?.plan === true);
  return (
    <ItemFrame item={item} label={t('timeline.plan.menu')}>
      <section aria-label={t('timeline.plan.title')} data-plan={item.id} className="flex flex-col gap-2 rounded-lg border border-border bg-surface px-3 py-2">
        <h3 className="flex items-baseline justify-between text-sm font-semibold text-text">
          {t(proposed ? 'timeline.plan.proposed' : 'timeline.plan.title')}
          {steps.length > 0 && <span className="text-xs font-normal text-text-subtle tabular-nums">{t('timeline.plan.progress', { done, total: steps.length })}</span>}
        </h3>
        {explanation !== null && explanation !== '' && (proposed
          ? <div className="max-h-(--diff-max-height) overflow-y-auto text-sm" data-plan-text><Body text={explanation} originThread={item.thread_id}/></div>
          : <div className="text-sm text-text-muted"><Body text={explanation} originThread={item.thread_id}/></div>)}
        {steps.length > 0 && <ul className="flex flex-col gap-1">
          {steps.map((step, index) => (
            <li key={index} className="flex items-start gap-2 text-sm">
              <span className="mt-0.5 shrink-0">
                {step.status === 'completed' ? (
                  <Check className="text-success" aria-hidden />
                ) : step.status === 'in_progress' ? (
                  <LoaderCircle className="animate-spin text-info" aria-hidden />
                ) : (
                  <Circle className="text-text-subtle" aria-hidden />
                )}
              </span>
              <div className={cx('min-w-0 flex-1', step.status === 'completed' ? 'text-text-muted line-through' : 'text-text')}><Body text={step.text} originThread={item.thread_id}/></div>
              <span className="sr-only">{t(`timeline.plan.status.${step.status}`)}</span>
            </li>
          ))}
        </ul>}
        {proposed && <ProposedPlanAnswer item={item} />}
      </section>
    </ItemFrame>
  );
}

/** Proceed and Keep planning under the plan a finished planning turn proposed, while nothing came after it. */
function ProposedPlanAnswer({ item }: { item: PlanItem }) {
  const threadId = item.thread_id;
  const read=useThreadReadRef(threadId);
  const thread = useThreadPart(read, (view) => view.thread);
  const passive=useThreadPart(read,(view)=>threadReadState(view).readOnly);
  const live=useSocketStatus().phase==='ready';
  const last = useThreadPart(read, (view) => {
    const turn = item.turn_id === null ? undefined : view.turns.get(item.turn_id);
    if (turn?.state !== 'completed' || view.thread?.current_turn_id !== null || view.thread.pending_inputs > 0) return false;
    return ![...view.turns.values()].some((other) => other.ordinal > turn.ordinal);
  });
  const settings = useQuery({ ...settingsQuery(threadId), enabled: last });
  const [lost,setLost]=useState<RequestDecisionStatus|null>(null);
  const answer=useMutation({mutationFn:(body:ThreadPlanDecideBody)=>decidePlan(read,item.id,body),onSuccess:(reply,body)=>{composerWriter.setPlanning(threadId,!body.proceed);threadWriter.apply(read,{inputs:[reply.input]});},onError:(error)=>{const current=sourceDecisionOf(error);if(current!==null)setLost(current);}});
  const decision=item.payload.decision??answer.data?.decision??lost;
  if(decision!==null)return <DecisionStatus source={decision}/>;
  if (!last || thread === null) return null;
  // The place may keep plans to the thread's managers; otherwise whoever may write to the agent answers.
  const managersOnly = settings.data?.find((setting) => setting.key === 'thread.plan_answered_by')?.value === 'managers';
  const may = holds(thread.access, managersOnly ? 'thread.manage' : 'thread.interact') && !thread.archived && !passive && live;
  return (
    <><PlanAnswer disabled={!may} pending={answer.isPending?(answer.variables.proceed?'proceed':'keep'):null}
      onProceed={() => answer.mutate({proceed:true})}
      onKeepPlanning={(text) => answer.mutate({proceed:false,text})}
    />{answer.error!==null&&<ErrorAlert error={answer.error}/>}</>
  );
}
