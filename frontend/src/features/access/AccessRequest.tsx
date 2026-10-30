/** Ask for everything that is missing in one step, with a reason; each part goes to its owner.
 *
 * What a person lacks to act is often more than one thing on more than one object. The refusal lists
 * all of it, and one request asks for all of it: one approval for each object, each sent to the people
 * who can give that thing. Before anything is sent the person sees exactly what will be asked, of
 * whom, and why it is needed. The three everyday choices are offered for the thing itself, with the
 * least that covers what was missing selected. A request that has been sent shows where it stands and can
 * be taken back. A refusal stays beside the form with its cause and the reason left in place.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Send } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { accessRequestQuery, cancelApproval, permissionsQuery, requestAccess, requestJoin } from '@/api/access';
import { ApiError } from '@/api/client';
import { approvalQuery } from '@/api/approvals';
import type { AccessChoice, AccessRequestCreateReply, Approval, MissingAccess } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RadioGroup } from '@/ui/RadioGroup';
import { DecisionStatus } from '@/features/approvals/Card';
import { ChoiceField } from '@/ui/ChoiceField';
import { Textarea } from '@/ui/Textarea';

import { CHOICE_ORDER, indexCatalogue, leastChoice } from './model';
import { usePermissionName } from './names';

/** The longest reason a request takes. */
const REASON_MAX = 500;
/** A pending answer is checked while the request is on screen, including expiry and partial grants. */

/** Where a request that was sent is remembered, so that the page shows it again after a reload. */
const REMEMBER = (objectId: string) => `orch.access.request.${objectId}`;

function remembered(objectId: string): AccessRequestCreateReply | null {
  try {
    const text = sessionStorage.getItem(REMEMBER(objectId));
    return text === null ? null : (JSON.parse(text) as AccessRequestCreateReply);
  } catch {
    return null;
  }
}

function remember(objectId: string, reply: AccessRequestCreateReply | null): void {
  try {
    if (reply === null) sessionStorage.removeItem(REMEMBER(objectId));
    else sessionStorage.setItem(REMEMBER(objectId), JSON.stringify(reply));
  } catch {
    // The page works without it: a request that was sent is simply not shown again after a reload.
  }
}

/** Where one item of a request stands, in a line: waiting, given, given in part with what was left out, or not given. */
function ItemLine({ approval }: { approval: Approval }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  if (approval.kind !== 'access') return null;
  const asked = approval.request.permissions;
  const what = `${approval.object?.name ?? ''} · ${asked.map(permissionName).join(', ')}`;
  const decision = (typeof approval.decision === 'object' && approval.decision !== null ? approval.decision : {}) as { permissions?: string[]; note?: string | null };
  const given = decision.permissions ?? asked;
  const left = asked.filter((name) => !given.includes(name));
  const line = approval.state === 'pending' ? t('access.request.line.pendingNobody', { what })
    : approval.state === 'approved' && left.length > 0 ? t('access.request.line.approvedFewer', { what, given: given.map(permissionName).join(', '), left: left.map(permissionName).join(', ') }) : what;
  return (
    <li className="flex flex-col gap-1">
      <span>{line}</span>
      {approval.state !== 'pending' && <DecisionStatus approval={approval} />}
      {approval.state !== 'denied' && decision.note !== undefined && decision.note !== null && decision.note !== '' && <span className="text-text-muted">{t('access.request.line.note', { note: decision.note })}</span>}
    </li>
  );
}

/** Who can give what is missing, as the refusal names them. */
function whoCanGive(missing: readonly MissingAccess[]): string {
  const names = new Set(missing.flatMap((entry) => entry.ask.map((person) => person.display_name)));
  return [...names].join(', ');
}

export function AccessRequest({ missing, objectId, forThread, fixed = false, privateReference = false, membership = false, sharedLabel, pendingId, onChanged }: { missing: readonly MissingAccess[]; objectId: string; forThread?: string; /** Asks for exactly what is missing: the three choices are not offered. */ fixed?: boolean; privateReference?: boolean; membership?:boolean; sharedLabel?:string; pendingId?: string | null; onChanged?: () => void }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName(!membership);
  const actKey = useActKey();
  const catalogue = useQuery({...permissionsQuery,enabled:!fixed&&!membership});
  const index = catalogue.data === undefined ? null : indexCatalogue(catalogue.data);
  const main = missing.find((entry) => entry.object.id === objectId) ?? missing[0];
  const askable = missing.filter((entry) => entry.closed === null);
  const kind = main?.object.kind ?? 'org';
  const [chosen, setChosen] = useState<AccessChoice | null>(null);
  const [reason, setReason] = useState('');
  const [existingId, setExistingId] = useState(pendingId ?? null);
  const existing = useQuery({ ...approvalQuery(existingId ?? ''), enabled: existingId !== null });
  const [sent, setSent] = useState<AccessRequestCreateReply | null>(() => privateReference ? null : remembered(objectId));
  const choice: AccessChoice = chosen ?? (index === null ? 'full_view' : leastChoice(index, kind, main?.permissions ?? []));

  const [attempt,setAttempt]=useState(0);
  const body = {
    items: askable.map((entry) => ({
      object: { kind: entry.object.kind, id: entry.object.id },
      ...(sharedLabel===undefined?{}:{label:sharedLabel}),
      permissions: !fixed && entry.object.id === objectId && index !== null && index.choice(choice, entry.object.kind).length > 0 ? index.choice(choice, entry.object.kind) : entry.permissions,
      ...(entry.options === null ? {} : { options: entry.options }),
    })),
    reason: reason.trim(),
    ...(forThread === undefined ? {} : { for_thread: forThread }),
  };
  const send = useMutation({
    mutationFn: () => membership ? requestJoin(objectId,{reason:body.reason,...(sharedLabel===undefined?{}:{label:sharedLabel})},actKey({body,attempt})) : requestAccess(body, actKey({body,attempt})),
    onSuccess: (reply) => {
      remember(objectId, reply);
      setSent(reply);
      onChanged?.();
    },
    onError: (error) => {
      if (error instanceof ApiError && error.is('conflict.exists') && typeof error.problem.approval_id === 'string') setExistingId(error.problem.approval_id);
    },
  });
  const state = useQuery({ ...accessRequestQuery(sent?.batch_id ?? ''), enabled: sent !== null });
  const seen = useRef('');
  useEffect(() => {
    const approvals = state.data?.approvals ?? (existing.data===undefined?undefined:[existing.data]);
    if (approvals === undefined) return;
    const signature = approvals.map((approval) => `${approval.id}:${approval.state}`).join(',');
    if (signature === seen.current) return;
    seen.current = signature;
    if (approvals.some((approval) => approval.state !== 'pending')) onChanged?.();
  }, [state.data, existing.data, onChanged]);
  const cancel = useMutation({
    mutationFn: async () => {
      for (const approval of existing.data === undefined ? sent?.approvals ?? [] : [existing.data]) await cancelApproval(approval.id);
    },
    onSuccess: () => {
      remember(objectId, null);
      setSent(null);
      setExistingId(null);
      setAttempt((value)=>value+1);
      send.reset();
      onChanged?.();
    },
  });

  const ask = whoCanGive(askable);
  if (askable.length === 0) return null;

  if (sent !== null || existingId !== null) {
    const approvals = existingId !== null ? existing.data===undefined?[]:[existing.data] : state.data?.approvals ?? sent?.approvals ?? [];
    const open = approvals.some((approval) => approval.state === 'pending');
    if (privateReference) return <div className="flex flex-col gap-2 text-sm text-text-muted" role="status">
      {approvals[0]!==undefined&&approvals[0].state!=='pending'?<DecisionStatus approval={approvals[0]} />:<span>{t('access.request.waiting')}</span>}
      {!open&&!existing.isPending&&approvals[0]!==undefined&&approvals[0].state!=='approved'&&<Button size="sm" onClick={()=>{remember(objectId,null);setSent(null);setExistingId(null);setAttempt((value)=>value+1);send.reset();}}>{t('access.request.askAgain')}</Button>}
      {(open || existing.isPending) && <Button size="sm" pending={cancel.isPending} disabled={existingId !== null && existing.data === undefined} onClick={() => cancel.mutate()}>{t('access.request.cancel')}</Button>}
      {cancel.error !== null && <ErrorAlert error={cancel.error} />}
    </div>;
    return (
      <div className="flex flex-col gap-2 text-left" role="status">
        <InlineAlert tone={state.data?.complete === true ? 'success' : 'info'}>
          {state.data?.complete === true ? t('access.request.complete') : ask === '' ? t('access.request.waiting') : t('access.request.waitingFor', { names: ask })}
        </InlineAlert>
        <ul className="flex flex-col gap-1 text-sm text-text">
          {approvals.map((approval) => (
            <ItemLine key={approval.id} approval={approval} />
          ))}
        </ul>
        {cancel.error !== null && <ErrorAlert error={cancel.error} />}
        {open && (
          <div>
            <Button size="sm" pending={cancel.isPending} onClick={() => cancel.mutate()}>
              {t('access.request.cancel')}
            </Button>
          </div>
        )}
      </div>
    );
  }

  const options = CHOICE_ORDER.filter((entry) => index === null || index.choice(entry, kind).length > 0).map((entry) => ({
    value: entry,
    name: t(`access.request.levels.${entry}`),
    description: t(`adminReach.requestChoices.${entry}`),
  }));
  const errorMessage = send.error instanceof ApiError && send.error.is('access.too_many_requests') && typeof send.error.problem.waiting === 'number'
    ? t('access.request.tooMany', { waiting: send.error.problem.waiting }) : undefined;

  return (
    <form
      className="flex w-full flex-col gap-3 text-left"
      onSubmit={(event) => {
        event.preventDefault();
        send.mutate();
      }}
    >
      {!fixed && <ChoiceField label={t('access.request.choose')} options={options}><RadioGroup<AccessChoice> label={t('access.request.choose')} value={choice} onValueChange={setChosen} options={options} /></ChoiceField>}
      {privateReference && <span className="text-sm text-text-muted">{membership?t('access.request.join'):t('access.request.asksFor',{access:main?.permissions.map(permissionName).join(', ')})}</span>}
      <Textarea
        label={t('access.request.reason')}
        {...(privateReference ? { optional: true, rows: 2 } : { hint: t('access.request.reasonHint', { max: REASON_MAX }) })}
        value={reason}
        maxLength={REASON_MAX}
        onChange={(event) => setReason(event.target.value)}
      />
      {send.error !== null && <ErrorAlert error={send.error} message={errorMessage} />}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {!privateReference && ask !== '' && <span className="min-w-0 flex-1 text-sm text-text-muted">{t('access.request.goesTo', { names: ask })}</span>}
        {privateReference && <span className="min-w-0 flex-1 text-sm text-text-muted">{t('people.reference.ownersAsked')}</span>}
        <Button type="submit" variant="primary" icon={<Send aria-hidden />} pending={send.isPending}>
          {t('access.request.send')}
        </Button>
      </div>
    </form>
  );
}
