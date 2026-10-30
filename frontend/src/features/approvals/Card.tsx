/** The frame an approval of the list is drawn in, whatever its kind, and the one line it becomes once decided.
 *
 * Each card says what kind of request it is (Agent, Capability, Access), its title, who asked, where it
 * comes from and how long ago. While it waits it has a warning border and its kind's own body and decision
 * controls inside; once decided it is one line, "Allowed once by Alice · 10:42", which opens to the request.
 * A request somebody else answered while the person looked at it says so.
 */
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { ChevronDown, ChevronRight, ShieldCheck, ShieldQuestionMark, ShieldX } from 'lucide-react';
import { createContext, type ReactNode, useContext, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { approvalOutcome } from '@/features/approvals/current';
import type { Approval, ApprovalKind, RequestDecisionStatus, UserRef } from '@/generated/api-types';
import { capabilityName } from '@/features/resources/capabilityWords';
import { useNodePath } from '@/realtime/stores/tree';
import { Badge } from '@/ui/Badge';
import { cx } from '@/ui/cx';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';

const HeadingLevel = createContext<2 | 3>(3);
const ApprovalSubject = createContext<string | null>(null);

/** The frame's visible subject, so a body can retain its field label without printing the subject again. */
export function useApprovalSubject(): string | null { return useContext(ApprovalSubject); }

/** A capability's wire key is never the subject of an ordinary request card or settled line. */
export function approvalTitle(approval: Approval, t: TFunction): string {
  if(approval.kind==='plan')return t('timeline.plan.proposed');
  if(approval.kind==='input')return t('approvals.heldInput');
  return approval.kind==='capability' ? capabilityName(approval.request.capability,approval.request.label,t) : approval.title;
}

/** A request frame exposes its one visible subject to the shared answer fields. */
export function ApprovalSubjectScope({ subject, children }: { subject: string; children: ReactNode }) {
  return <ApprovalSubject value={subject}>{children}</ApprovalSubject>;
}

/** Lists immediately below a page heading use h2; cards in named sections keep h3. */
export function ApprovalHeadings({ level, children }: { level: 2 | 3; children: ReactNode }) {
  return <HeadingLevel value={level}>{children}</HeadingLevel>;
}

/** The word for a kind of approval. */
export function kindWord(kind: ApprovalKind | 'port', t: TFunction): string {
  switch (kind) {
    case 'port': return t('ports.requestTitle');
    case 'plan': return t('approvals.kind.plan');
    case 'input': return t('approvals.kind.input');
    case 'harness':
      return t('approvals.kind.harness');
    case 'capability':
      return t('approvals.kind.capability');
    case 'access':
      return t('approvals.kind.access');
  }
}

/** What a decision came to, in the words of the option that was chosen where there was one. */
export function decidedWords(approval: Approval, t: TFunction): string {
  const application = capabilityApplication(approval);
  if (application?.state === 'failed') return t('approvals.capability.notGranted');
  if (application?.state === 'applying') return t('approvals.capability.applying');
  if(approval.kind==='harness'&&approval.request.request_kind==='plan') {const option=(approval.decision as {option_id?:string}|null)?.option_id;if(option==='proceed'||option==='keep_planning')return t(`approvals.source.${option}`);}
  if(approval.kind==='input'&&approval.state==='approved')return t('approvals.source.run');
  if(approval.kind==='plan'&&['approved','answered'].includes(approval.state))return t((approval.decision as {option_id?:string}|null)?.option_id==='keep_planning'?'approvals.source.keep_planning':'approvals.source.proceed');
  if (approval.kind === 'harness') {
    const decision = approval.decision as { option_id?: string } | null;
    const option = approval.request.options.find((candidate) => candidate.id === decision?.option_id);
    if (option !== undefined) {
      // What was done, not what the button said: "Allowed once", where the option was to allow once.
      if (option.style === 'allow' && option.scope === 'once') return t('approvals.allowedOnce');
      if (option.style === 'allow' && option.scope === 'session') return t('approvals.allowedSession');
      if (option.style === 'deny' && (option.scope === null || option.scope === 'once')) return t('approvals.denied');
      return option.label;
    }
  }
  switch (approvalOutcome(approval)) {
    case 'answered': return t('approvals.answered');
    case 'approved':
      return approval.kind === 'harness' ? t('approvals.allowed') : t('approvals.approved');
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

/** Consent and the result of applying a capability are separate outcomes. */
function capabilityApplication(approval: Approval | undefined): { state?: string; reason?: string | null } | null {
  if (approval?.kind !== 'capability' || approval.state !== 'approved') return null;
  return (approval.decision as { application?: { state?: string; reason?: string | null } } | null)?.application ?? null;
}

/** Already-authorized notice metadata uses the same status line as the decision's own card. */
export interface DecisionLine {
  word:string;
  by:UserRef|null;
  at:string|null;
  given:boolean;
  expiresAt:string|null;
  note:string|null;
}

/** One shared settled status: outcome, deciding person, duration, time and any denial note. */
export function DecisionStatus({approval,source,line}:{approval:Approval;source?:never;line?:never}|{source:RequestDecisionStatus;approval?:never;line?:never}|{line:DecisionLine;approval?:never;source?:never}) {
  const {t}=useTranslation();
  const format=useFormatContext();
  const decision=approval?.decision !== null && typeof approval?.decision === 'object' ? approval.decision as {expires_at?:string|null;note?:string|null} : null;
  const given=line?.given??(approval?.kind==='access' && approval.state==='approved');
  const by=line?.by??source?.decided_by ?? approval?.decided_by ?? null;
  const at=line?.at??source?.decided_at ?? approval?.decided_at ?? null;
  const expires=line?.expiresAt??decision?.expires_at??null;
  const duration=given && expires!==null && at !== null ? Math.max(0,Date.parse(expires)-Date.parse(at)) : null;
  const note=line?.note??(approval?.state==='denied'?decision?.note:null);
  const application=capabilityApplication(approval);
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-x-1 gap-y-1 text-sm text-text-muted" role="status">
    <span>{line!==undefined?line.word:source!==undefined?(source.action===null?t(`approvals.${source.state==='cancelled'?'cancelled':source.state==='expired'?'expired':'answered'}`):t(`approvals.source.${source.action}`)):given?t('approvals.quick.given'):decidedWords(approval,t)}</span>
    {approval?.decided_automatically === true ? <span>{t('approvals.automatically')}</span> : by !== null && <><span>{t('approvals.quick.by')}</span><UserChip user={by} size={24} /></>}
    {given && <><span aria-hidden>·</span><span>{duration===null?t('approvals.quick.always'):t('approvals.quick.forDuration',{duration:formatSpan(duration,format,'long')})}</span></>}
    {at !== null && <><span aria-hidden>·</span><RelativeTime at={at} /></>}
    {note!==undefined&&note!==null&&note!==''&&<span className="wrap-anywhere">{note}</span>}
    {application?.state==='failed'&&application.reason&&<span className="wrap-anywhere text-danger">{application.reason}</span>}
  </span>;
}

/**
 * Who asked: a person, or the agent of a thread. `placed` says that the place is named right beside
 * (the thread itself, as the last of its path), so that the thread is not named twice.
 */
export function Requester({ approval, placed = false }: { approval: Approval; placed?: boolean }) {
  const { t } = useTranslation();
  if ('user' in approval.requester) {
    return (
      <span className="inline-flex items-center gap-2">
        <span className="text-text-muted">{t('approvals.askedBy')}</span>
        <UserChip user={approval.requester.user} size={24} />
      </span>
    );
  }
  return <span className="text-text-muted">{placed ? t('approvals.askedByTheAgent') : t('approvals.askedByAgent', { thread: approval.requester.thread.name })}</span>;
}

/** Where a request comes from, as the names from the project down to the thread; `null` where nothing names a place. */
function useWhere(approval: Approval): string | null {
  const { t } = useTranslation();
  const { path } = useNodePath(approval.thread_id ?? approval.node?.id);
  const names = path.filter((node) => node.kind !== 'org').map((node) => node.name);
  const named = approval.node?.name;
  if (names.length === 0 && named !== undefined && named !== null && named !== '') names.push(named);
  return names.length === 0 ? null : names.join(t('navigation.pathJoin'));
}

/** The frame of a request that waits. */
export function CardFrame({
  approval,
  title,
  quiet = false,
  badges,
  children,
}: {
  approval: Approval;
  /** What the card is headed with, where the request's own title is not it: several requests together. */
  title?: string;
  /** Nothing waits in it any more: no warning border. */
  quiet?: boolean;
  /** More badges beside the kind: the risk of a capability. */
  badges?: ReactNode;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const where = useWhere(approval);
  const Title = useContext(HeadingLevel) === 2 ? 'h2' : 'h3';
  const subject = title ?? approvalTitle(approval,t);
  // Where the thread that asked is the place named beside, it is not named twice.
  const asker = 'thread' in approval.requester ? approval.requester.thread.name : null;
  const placed = asker !== null && (where?.endsWith(asker) ?? false);
  return (
    <ApprovalSubjectScope subject={subject}><section
      data-approval={approval.id}
      data-kind={approval.kind}
      data-tutorial-target="approval-card"
      aria-label={subject}
      className={cx('flex flex-col gap-3 rounded-lg bg-surface px-3 py-3 sm:px-4', quiet ? 'border border-border' : 'border-2 border-warning')}
    >
      <header className="flex items-start gap-3">
        <ShieldQuestionMark className={cx('mt-0.5 shrink-0', quiet ? 'text-text-subtle' : 'text-warning')} aria-hidden />
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <Badge tone="neutral">{kindWord(approval.kind, t)}</Badge>
            {badges}
            <span className="ml-auto text-xs text-text-subtle">
              {t('approvals.requested')} <RelativeTime at={approval.created_at} />
            </span>
          </div>
          <Title className="text-base font-semibold wrap-anywhere text-text">{subject}</Title>
          <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
            <Requester approval={approval} placed={placed} />
            {where !== null && (approval.thread_id === null ? (
                  <span className="text-text-muted">{where}</span>
                ) : (
                  <Link
                    to="/thread/$threadId"
                    params={{ threadId: approval.thread_id }}
                    aria-label={t('approvals.openNewThread', { name: where })}
                    className="rounded-md text-text-muted underline-offset-2 hover:text-text hover:underline"
                  >
                    {where}
                  </Link>
                ))}
          </p>
        </div>
      </header>
      {children}
    </section></ApprovalSubjectScope>
  );
}

/**
 * A request that is decided: one line, which opens to the request. `meanwhile` marks one that was
 * answered while the person had it in front of them as waiting; `mine` that they answered it.
 */
export function DecidedCard({ approval, meanwhile = false, children }: { approval: Approval; meanwhile?: boolean; children?: ReactNode }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const what = decidedWords(approval, t);
  const application = capabilityApplication(approval);
  const successful = ['approved', 'answered'].includes(approvalOutcome(approval)) && application?.state !== 'failed' && application?.state !== 'applying';
  const Icon = successful ? ShieldCheck : approval.state === 'pending' || application?.state === 'applying' ? ShieldQuestionMark : ShieldX;
  return (
    <section data-approval={approval.id} data-kind={approval.kind} data-state={approval.state} aria-label={approval.title} className="flex flex-col gap-2 rounded-lg border border-border bg-surface px-3 py-2">
      <div className="flex flex-wrap items-center gap-2"><button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((on) => !on)}
        className="focus-inset flex w-full items-center gap-2 rounded-md text-left text-sm text-text-muted hover:text-text"
      >
        {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
        <Icon className={cx('shrink-0', successful ? 'text-success' : application?.state === 'failed' ? 'text-danger' : 'text-text-subtle')} aria-hidden />
        <span className="flex min-w-0 flex-1 flex-wrap gap-x-3 gap-y-1">
          <span className="font-medium wrap-anywhere text-text">{approvalTitle(approval,t)}</span><span className="sr-only">{what}</span>
        </span>
      </button><DecisionStatus approval={approval} /></div>
      {meanwhile && (
        <p role="status" className="text-sm text-text-muted">
          {t('approvals.quick.alreadyDecided')}
        </p>
      )}
      {open && <div className="flex flex-col gap-2 text-sm text-text-muted">{children}</div>}
    </section>
  );
}
