/** Harness request and decision card.
 *
 * What the agent asks before it acts, as a card in the conversation where it was asked. While it
 * waits the card has a warning border, the request in full and one button for each answer; for
 * somebody who may not answer it says who can. Once it is decided it is one line (who decided, and
 * what, and when), which opens to the request again; a decision that came too late says who got
 * there first. Answering waits for the live connection, whose state it depends on.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, ShieldCheck, ShieldQuestionMark, ShieldX } from 'lucide-react';
import type { TFunction } from 'i18next';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { asPortApproval, PortApprovalCard } from '@/features/ports/Request';
import { viewedItemQuery } from '@/api/threadHistory';
import { RawCall } from '@/features/timeline/RawCall';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { approvalKey, approvalQuery } from '@/api/approvals';
import type { Approval, Item } from '@/generated/api-types';
import { useDecision } from '@/features/approvals/Decision';
import { DecisionError } from '@/features/approvals/DecisionError';
import { approvalTitle, ApprovalSubjectScope } from '@/features/approvals/Card';
import { approvalAtItem, approvalOutcome } from '@/features/approvals/current';
import { approvalSnapshot } from '@/features/approvals/snapshot';
import { AnswersForm, decisionOfOption, ElicitationForm, type HarnessApproval, OptionButtons, optionWords, planDecision, RequestBody } from '@/features/approvals/Harness';
import { PlanAnswer } from '@/features/approvals/PlanAnswer';
import { useTimelineContext } from '@/features/timeline/context';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { useThreadPart, useThreadView } from '@/realtime/stores/thread';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { TimeOfDay } from '@/ui/TimeOfDay';

type ApprovalItem = Extract<Item, { kind: 'approval' }>;

/** What the decision came to, in the words of the option that was chosen. */
function decidedWhat(approval: Approval, t: TFunction): string {
  if (approval.kind === 'harness') {
    const decision = approval.decision as { option_id?: string } | null;
    const option = approval.request.options.find((candidate) => candidate.id === decision?.option_id);
    if (option !== undefined) return optionWords(option, t);
  }
  switch (approvalOutcome(approval)) {
    case 'answered': return t('approvals.answered');
    case 'approved': return t('approvals.allowed');
    case 'denied':
      return t('approvals.denied');
    case 'expired':
      return t('approvals.expired');
    case 'cancelled':
      return t('approvals.cancelled');
    case 'pending':
      return t('approvals.waiting');
  }
}

function DecidedLine({ approval, lostTo, children }: { approval: Approval; lostTo: Approval | null; children?: React.ReactNode }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const what = decidedWhat(approval, t);
  const Icon = ['approved', 'answered'].includes(approvalOutcome(approval)) ? ShieldCheck : approval.state === 'pending' ? ShieldQuestionMark : ShieldX;
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface px-3 py-2">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((on) => !on)}
        className="flex w-full items-center gap-2 rounded-md text-left text-sm text-text-muted hover:text-text"
      >
        {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
        <Icon className={['approved', 'answered'].includes(approvalOutcome(approval)) ? 'text-success' : 'text-text-subtle'} aria-hidden />
        <span className="flex min-w-0 flex-1 flex-wrap gap-x-3 gap-y-1">
          <span className="font-medium wrap-anywhere text-text">{approvalTitle(approval,t)}</span><span>{what}
          {approval.decided_automatically ? ` ${t('approvals.automatically')}` : approval.decided_by !== null ? ` ${t('approvals.by', { name: approval.decided_by.display_name })}` : ''}
          </span>{approval.decided_at !== null && <TimeOfDay at={approval.decided_at} />}
        </span>
      </button>
      {lostTo !== null && (
        <p role="status" className="text-sm text-text-muted">
          {t('approvals.alreadyAnswered', { name: lostTo.decided_by?.display_name ?? t('approvals.someoneElse') })}
        </p>
      )}
      {open && children}
    </div>
  );
}

/** A question's confirmed answers remain readable after its editable form closes. */
function QuestionAnswers({ approval }: { approval: HarnessApproval }) {
  const { t } = useTranslation();
  if (approval.request.request_kind !== 'user_input') return null;
  const decision = approval.decision;
  const answers: unknown = typeof decision === 'object' && decision !== null && 'answers' in decision ? decision.answers : undefined;
  return <dl className="flex flex-col gap-2 text-sm">{approval.request.detail.questions.map((question) => {
    const value: unknown = typeof answers === 'object' && answers !== null && question.id in answers ? (answers as Record<string, unknown>)[question.id] : undefined;
    const words = Array.isArray(value) ? value.filter((word): word is string => typeof word === 'string').join(', ') : '';
    return <div key={question.id}>{question.question !== approval.title && <dt className="font-medium">{question.question}</dt>}<dd className="text-text-muted">{words === '' ? t('approvals.unanswered') : words}</dd></div>;
  })}</dl>;
}

function HarnessPending({ approval, itemId, threadId, onLost }: { approval: HarnessApproval; itemId: string; threadId: string; onLost: (approval: Approval) => void }) {
  const { t } = useTranslation();
  const { live } = useTimelineContext();
  const owner = useThreadPart(threadId, (view) => view.thread?.owner ?? null);
  const decision = useDecision(approval.id, { onLost });
  const request = approval.request;
  const may = approval.can_decide && live;
  const busy = decision.pending;
  return (
    <ApprovalSubjectScope subject={approval.title}><section
      aria-label={approval.title}
      data-approval={itemId}
      data-tutorial-target="approval-card"
      className={cx('flex flex-col gap-3 rounded-lg border-2 border-warning bg-surface px-3 py-3')}
    >
      <header className="flex items-start gap-2">
        <ShieldQuestionMark className="mt-0.5 text-warning" aria-hidden />
        <div className="flex min-w-0 flex-1 flex-col">
          <h3 className="text-base font-semibold text-text">{approval.title}</h3>
          <p className="text-xs text-text-subtle">
            {t('approvals.requested')} <RelativeTime at={approval.created_at} />
          </p>
        </div>
      </header>
      <RequestBody request={request} />
      {request.request_kind === 'user_input' ? (
        <AnswersForm
          questions={request.detail.questions}
          disabled={!may || busy}
          pending={busy}
          onSend={(answers) => decision.decide({ answers }, t('approvals.sendAnswers'))}
        />
      ) : request.request_kind === 'mcp_elicitation' ? (
        <ElicitationForm
          schema={request.detail.schema}
          message={request.detail.message}
          disabled={!may || busy}
          pending={busy}
          onSend={(action, content) => decision.decide({ action, ...(content === undefined ? {} : { content }) }, t('approvals.accept'))}
        />
      ) : request.request_kind === 'plan' ? (
        <PlanAnswer disabled={!may} pending={decision.pressed === t('plan.proceed') ? 'proceed' : decision.pressed === t('plan.keepPlanning') ? 'keep' : null}
          onProceed={() => decision.decide(planDecision(true), t('plan.proceed'))}
          onKeepPlanning={(words) => decision.decide(planDecision(false, words), t('plan.keepPlanning'))} />
      ) : (
        <OptionButtons target={request.request_kind === 'command' ? request.detail.command : approval.title}
          options={request.options}
          disabled={!may || busy}
          pressed={decision.pressed}
          onChoose={(option) => decision.decide(decisionOfOption(option), option.label)}
        />
      )}
      {!approval.can_decide && (
        <p className="text-sm text-text-muted">
          {owner === null ? t('approvals.waitingForAnswer') : t('approvals.waitingForAnswerBy', { names: owner.display_name })}
        </p>
      )}
      {approval.can_decide && !live && <p className="text-sm text-text-muted">{t('approvals.waitForLine')}</p>}
      {decision.error !== null && <DecisionError error={decision.error} />}
    </section></ApprovalSubjectScope>
  );
}

function CapabilityPending({ approval, itemId, onLost }: { approval: Extract<Approval, { kind: 'capability' }>; itemId: string; onLost: (approval: Approval) => void }) {
  const { t } = useTranslation();
  const { live } = useTimelineContext();
  const decision = useDecision(approval.id, { onLost });
  const request = approval.request;
  const title = approvalTitle(approval,t);
  return (
    <section aria-label={title} data-approval={itemId} className="flex flex-col gap-3 rounded-lg border-2 border-warning bg-surface px-3 py-3">
      <header className="flex items-start gap-2">
        <ShieldQuestionMark className="mt-0.5 text-warning" aria-hidden />
        <div className="flex min-w-0 flex-1 flex-col">
          <h3 className="text-base font-semibold text-text">{title}</h3>
          <p className="text-xs text-text-subtle">
            {t('approvals.requested')} <RelativeTime at={approval.created_at} />
          </p>
        </div>
      </header>
      <p className="text-sm text-text">{request.reason}</p>
      {request.consequences.length > 0 && (
        <ul className="list-disc pl-6 text-sm text-text-muted">
          {request.consequences.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button action="approve" aria-label={actionName('approve', title)} variant="secondary" pending={decision.pressed === t('approvals.approve')} disabled={!approval.can_decide || !live || decision.pending} onClick={() => decision.decide({ approve: true }, t('approvals.approve'))}>
          {t('approvals.approve')}
        </Button>
        <Button action="deny" aria-label={actionName('deny', title)} variant="danger-ghost" disabled={!approval.can_decide || !live || decision.pending} onClick={() => decision.decide({ approve: false }, t('approvals.refuse'))}>
          {t('approvals.refuse')}
        </Button>
      </div>
      {decision.error !== null && <DecisionError error={decision.error} />}
    </section>
  );
}

/** The source stays in the same admitted history and is fetched only when its work detail is opened. */
function NativeQuestionWorkBody({ item, sourceId }: { item: ApprovalItem; sourceId: string }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(read);
  const held = view.items.get(sourceId);
  const options = viewedItemQuery(read, sourceId);
  const query = useQuery({ ...options, enabled: held === undefined && view.thread !== null && view.closed === null && options.enabled });
  if (view.closed !== null) return <p className="text-sm text-text-muted">{t('threadViews.unavailable')}</p>;
  const source = held ?? query.data;
  if (source === undefined) return query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : <SkeletonGroup><Skeleton className="h-10 w-full" /></SkeletonGroup>;
  if (source.kind !== 'tool_call' || source.thread_id !== item.thread_id || source.turn_id !== item.turn_id) return <p className="text-sm text-text-muted">{t('threadViews.unavailable')}</p>;
  return <RawCall item={source} />;
}

function NativeQuestionWork({ item }: { item: ApprovalItem }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const sourceId = item.parent_item_id;
  if (sourceId === null) return null;
  return <details className="mt-2 rounded-lg border border-border px-3 py-2" onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm text-text-muted">{t('thread.header.workDetail')}</summary>
    {open && <div className="pt-3"><NativeQuestionWorkBody item={item} sourceId={sourceId} /></div>}
  </details>;
}

export function ApprovalCard({ item }: { item: ApprovalItem }) {
  const { t } = useTranslation();
  const { threadId, live } = useTimelineContext();
  const queryClient = useQueryClient();
  const id = item.payload.approval_id;
  const inline = approvalSnapshot(item.payload.approval, id);
  const query = useQuery({ ...approvalQuery(id), enabled: inline === undefined });
  const first = useRef(true);
  const [lostTo, setLostTo] = useState<Approval | null>(null);
  useEffect(() => {
    // Other views observe this same entry without fetching. Never overwrite a confirmed decision
    // with a pending item that was delivered before the person's POST returned.
    if (inline !== undefined && inline.state !== 'pending') queryClient.setQueryData(approvalKey(id), inline);
  }, [inline, id, queryClient]);
  useEffect(() => {
    if (inline !== undefined) return;
    // The item's status mirrors the approval's: when it changes, somebody decided, and the card is read again.
    if (first.current) {
      first.current = false;
      return;
    }
    void queryClient.invalidateQueries({ queryKey: approvalKey(id) });
  }, [item.status, item.seq_updated, id, inline, queryClient]);

  // A confirmed local decision wins over an older pending item until its live replacement arrives.
  const approval = approvalAtItem(item, query.data);
  const portApproval = asPortApproval(approval);
  let body: React.ReactNode;
  if (approval === undefined) {
    body = query.isSuccess ? <ErrorState compact error={null} message={t('approvals.incomplete')}
      action={<Button size="sm" pending={query.isRefetching} onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} /> : query.isError ? (
      <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
    ) : (
      <SkeletonGroup className="flex flex-col gap-2 rounded-lg border border-border px-3 py-3">
        <Skeleton className="h-4 w-1/3" />
        <Skeleton className="h-10 w-full" />
      </SkeletonGroup>
    );
  } else if (portApproval !== null) {
    body = <PortApprovalCard approval={portApproval} live={live} />;
  } else if (approval.state === 'pending' && item.status === 'pending') {
    body =
      approval.kind === 'harness' ? (
        <HarnessPending approval={approval} itemId={item.id} threadId={threadId} onLost={setLostTo} />
      ) : approval.kind === 'capability' ? (
        <CapabilityPending approval={approval} itemId={item.id} onLost={setLostTo} />
      ) : (
        <InlineAlert tone="warning" title={approval.title}>
          {t('approvals.elsewhere')}
        </InlineAlert>
      );
  } else {
    body = (
      <DecidedLine approval={approval} lostTo={lostTo}>
        {approval.kind === 'harness' && (approval.request.request_kind === 'user_input' ? <QuestionAnswers approval={approval} /> : <RequestBody request={approval.request} />)}
      </DecidedLine>
    );
  }
  return (
    <ItemFrame item={item} label={t('timeline.approval.menu')}>
      {body}
      {approval?.kind === 'harness' && (approval.request.request_kind === 'user_input' || approval.request.request_kind === 'mcp_elicitation') && <NativeQuestionWork item={item} />}
    </ItemFrame>
  );
}
