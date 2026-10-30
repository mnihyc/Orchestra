/** Risk, the scope that decides (host, org or system) and recreation consequences.
 *
 * What an environment asks to be given beyond the ordinary, as the person who may decide it sees it: how
 * risky it is, which environment asks and on which host it would open something up, why, what granting it
 * would do, and whose approvers decide it. Approve, or Deny, each with a note for whoever asked if the
 * approver wants to leave one. Answering waits for the live connection, whose state it depends on.
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { CardFrame } from '@/features/approvals/Card';
import { useDecision } from '@/features/approvals/Decision';
import { capabilityConsequence, capabilityName } from '@/features/resources/capabilityWords';
import type { Approval } from '@/generated/api-types';
import { Badge, type BadgeTone } from '@/ui/Badge';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Textarea } from '@/ui/Textarea';

export type CapabilityApproval = Extract<Approval, { kind: 'capability' }>;

const RISK_TONE: Record<CapabilityApproval['request']['risk'], BadgeTone> = {
  low: 'neutral',
  medium: 'warning',
  high: 'danger',
  critical: 'danger',
};

export function CapabilityCard({ approval, live, onSettled }: { approval: CapabilityApproval; live: boolean; onSettled: (approval: Approval) => void }) {
  const { t } = useTranslation();
  const decision = useDecision(approval.id, { onDecided: onSettled, onLost: onSettled });
  const [note, setNote] = useState('');
  const request = approval.request;
  const label = capabilityName(request.capability, request.label, t);
  const may = approval.can_decide && live;
  const busy = decision.pending;
  const send = (approve: boolean, label: string) => decision.decide({ approve, ...(note.trim() === '' ? {} : { note: note.trim() }) }, label);

  return (
    <CardFrame approval={approval} title={label} badges={<Badge tone={RISK_TONE[request.risk]}>{t(`approvals.risk.${request.risk}`)}</Badge>}>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-text-muted">{t('approvals.capability.environment')}</dt>
        <dd className="min-w-0 wrap-anywhere text-text">
          {request.environment.name}
        </dd>
        <dt className="text-text-muted">{t('approvals.capability.host')}</dt>
        <dd className="min-w-0 wrap-anywhere text-text">
          {request.host.name}
        </dd>
        {request.reason !== '' && (
          <>
            <dt className="text-text-muted">{t('approvals.capability.why')}</dt>
            <dd className="min-w-0 wrap-anywhere text-text">{request.reason}</dd>
          </>
        )}
      </dl>
      {request.consequences.length > 0 && (
        <div className="flex flex-col gap-1">
          <p className="text-sm font-medium text-text">{t('approvals.capability.would')}</p>
          <ul className="list-disc pl-6 text-sm text-text-muted">
            {request.consequences.map((line) => (
              <li key={line}>{capabilityConsequence(line, t)}</li>
            ))}
          </ul>
        </div>
      )}
      {approval.approver_scope !== null && <p className="text-sm text-text-muted">{t(`approvals.scope.${approval.approver_scope}`)}</p>}
      {approval.can_decide && <Textarea label={t('approvals.note')} rows={2} value={note} disabled={busy} onChange={(event) => setNote(event.target.value)} />}
      <div className="flex flex-wrap items-center gap-2">
        <Button action="approve" aria-label={actionName('approve', label)} variant="secondary" pending={decision.pressed === t('approvals.approve')} disabled={!may || busy} onClick={() => send(true, t('approvals.approve'))}>
          {actionName('approve', label)}
        </Button>
        <Button action="deny" aria-label={actionName('deny', label)} variant="danger-ghost" pending={decision.pressed === t('approvals.deny')} disabled={!may || busy} onClick={() => send(false, t('approvals.deny'))}>
          {actionName('deny', label)}
        </Button>
      </div>
      {!approval.can_decide && <p className="text-sm text-text-muted">{t('approvals.cannotAnswer')}</p>}
      {approval.can_decide && !live && <p className="text-sm text-text-muted">{t('approvals.waitForLine')}</p>}
      {decision.error !== null && <ErrorAlert error={decision.error} />}
    </CardFrame>
  );
}
