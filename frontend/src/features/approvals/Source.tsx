/** Plans and held inputs use the same canonical approval decisions in the Inbox, the list and their source cards. */
import { useTranslation } from 'react-i18next';
import type { Approval } from '@/generated/api-types';
import { CardFrame, DecidedCard } from '@/features/approvals/Card';
import { PlanAnswer } from '@/features/approvals/PlanAnswer';
import { planDecision } from '@/features/approvals/Harness';
import { useDecision } from '@/features/approvals/Decision';
import { DecisionError } from '@/features/approvals/DecisionError';
import { Body } from '@/features/chat/Body';
import { queuePreview } from '@/features/composer/queue';
import { Button } from '@/ui/Button';

export type SourceApproval=Extract<Approval,{kind:'plan'|'input'}>;

export function SourceBrief({approval}:{approval:SourceApproval}) {
  const text=approval.kind==='plan' ? [approval.request.plan.explanation??'',...approval.request.plan.steps.map((step)=>step.text)].filter(Boolean).join('\n\n') : queuePreview(approval.request.content);
  return <div className="text-sm wrap-anywhere"><Body text={text} {...(approval.thread_id===null?{}:{originThread:approval.thread_id})}/></div>;
}

export function SourceCard({approval,live,onSettled}:{approval:SourceApproval;live:boolean;onSettled:(approval:Approval)=>void}) {
  const {t}=useTranslation();
  const decision=useDecision(approval.id,{onDecided:onSettled,onLost:onSettled});
  if(approval.state!=='pending')return <DecidedCard approval={approval}><SourceBrief approval={approval}/></DecidedCard>;
  const may=approval.can_decide&&live;
  return <CardFrame approval={approval}><SourceBrief approval={approval}/>
    {approval.kind==='plan'?<PlanAnswer disabled={!may} pending={decision.pending?(decision.pressed==='proceed'?'proceed':'keep'):null} onProceed={()=>decision.decide(planDecision(true),'proceed')} onKeepPlanning={(words)=>decision.decide(planDecision(false,words),'keep')}/>:<Button size="sm" variant="primary" className="self-start" disabled={!may} pending={decision.pending} onClick={()=>decision.decide({run:true},'run')}>{t('composer.queue.runIt')}</Button>}
    {!may&&<span className="text-sm text-text-muted">{t(!live?'approvals.waitForLine':'approvals.cannotAnswer')}</span>}
    {decision.error!==null&&<DecisionError error={decision.error}/>}</CardFrame>;
}
