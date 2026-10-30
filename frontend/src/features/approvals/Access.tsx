/** One card for the parts of a request: the permissions, options and expiry of each, decided one by one, with what opens a terminal or reaches a person's machine said in words.
 *
 * A person who asks for access to several things at once is one card with a row for each thing this approver
 * decides. A row says what is asked and why, with every permission asked for ticked (untick what is not to be
 * given), the models and efforts of a provider the same way, when what is given ends, and a note; Approve gives
 * what is chosen, Deny refuses. Whatever opens a terminal or reaches somebody's machine is said in words on
 * the row before it is approved, and never only as a tick.
 */
import { ShieldCheck, ShieldX } from 'lucide-react';
import { useState } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { CardFrame, DecisionStatus } from '@/features/approvals/Card';
import { useDecision } from '@/features/approvals/Decision';
import { usePermissionName } from '@/features/access/names';
import type { Approval } from '@/generated/api-types';
import { actionName } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import { UserChip } from '@/ui/UserChip';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useNow } from '@/lib/time';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';

export type AccessApproval = Extract<Approval, { kind: 'access' }>;

/** The permissions that open a terminal or reach somebody's machine, and the sentence that says what each lets a person do. */
const REACH = {
  'terminal.manage': 'approvals.reach.terminal_manage',
  'terminal.view': 'approvals.reach.terminal_view',
  'files.manage': 'approvals.reach.files_manage',
  'files.view': 'approvals.reach.files_view',
  'environment.manage': 'approvals.reach.environment_manage',
  'environment.use': 'approvals.reach.environment_use',
  'host.use': 'approvals.reach.host_use',
  'mount.manage': 'approvals.reach.mount_manage',
  'provider.use': 'approvals.reach.provider_use',
} as const;

type ReachKey = (typeof REACH)[keyof typeof REACH];

/** The sentences for what a set of permissions reaches, each once, in the order the permissions come. */
export function reachOf(permissions: Iterable<string>): ReachKey[] {
  const said: ReachKey[] = [];
  for (const permission of permissions) {
    const key = (REACH as Partial<Record<string, ReachKey>>)[permission];
    if (key !== undefined && !said.includes(key)) said.push(key);
  }
  return said;
}

/** When what is given ends, by how long from the moment of the decision. */
const ENDS = { never: null, day: 1, week: 7, month: 30 } as const;
type Ends = keyof typeof ENDS | 'custom';
const DAY_MS = 24 * 60 * 60 * 1000;

/** The same one-click duration decisions in the Inbox, permission page and reference card. */
export function QuickGrant({ approval, live = true, onSettled, onOther }: { approval: AccessApproval; live?: boolean; onSettled: (approval: Approval) => void; onOther: () => void }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const permissionName = usePermissionName(!approval.request.membership);
  const decision = useDecision(approval.id, {onDecided:onSettled,onLost:onSettled});
  const [denying, setDenying] = useState(false);
  const [note, setNote] = useState('');
  const shipped = SETTINGS_REGISTRY.find((entry) => entry.key === 'access.quick_grant_durations')?.default;
  const durations = approval.quick_grant_durations ?? (Array.isArray(shipped) ? shipped.filter((value):value is string=>typeof value==='string') : []);
  const user = approval.request.holder.kind === 'user' ? approval.request.holder.user : null;
  const blocked = !approval.can_decide || !live || decision.pending;
  return <div className="flex flex-col gap-2">
    <div className="flex flex-wrap items-center gap-1 text-sm text-text-muted"><span>{approval.request.membership?t('approvals.quick.join'):t('approvals.quick.give', { access: approval.request.permissions.map(permissionName).join(', ') })}</span>{user !== null && <UserChip user={user} size={24} />}</div>
    <div className="flex flex-wrap items-center gap-1">
      {durations.map((duration) => {
        const days = duration.endsWith('d') ? Number(duration.slice(0,-1)) : null;
        const label = duration === 'always' ? t('approvals.quick.always') : days !== null && Number.isFinite(days) && days > 0 ? formatSpan(days * DAY_MS, format) : null;
        if (label === null) return null;
        return <Button key={duration} size="sm" disabled={blocked} pending={decision.pressed === duration} onClick={() => decision.decide({approve:true,quick_duration:duration},duration)}>{label}</Button>;
      })}
      <Button size="sm" variant="danger-ghost" disabled={blocked} onClick={() => setDenying((open)=>!open)}>{t('approvals.deny')}</Button>
      <Button size="sm" variant="ghost" disabled={blocked} onClick={onOther}>{t('approvals.quick.other')}</Button>
    </div>
    {denying && <form className="flex flex-col gap-2" onSubmit={(event)=>{event.preventDefault();decision.decide({approve:false,...(note.trim()===''?{}:{note:note.trim()})},'deny');}}><Textarea label={t('approvals.note')} optional rows={2} maxLength={500} value={note} onChange={(event)=>setNote(event.target.value)} /><Button type="submit" size="sm" variant="danger-ghost" disabled={blocked} pending={decision.pressed==='deny'}>{t('approvals.deny')}</Button></form>}
    {decision.error !== null && <ErrorAlert error={decision.error} />}
  </div>;
}

function kindWord(kind: string, t: TFunction): string {
  switch (kind) {
    case 'thread':
      return t('approvals.permissionKind.thread');
    case 'topic':
      return t('approvals.permissionKind.topic');
    case 'project':
      return t('approvals.permissionKind.project');
    case 'org':
      return t('approvals.permissionKind.org');
    case 'provider':
      return t('approvals.permissionKind.provider');
    case 'environment':
      return t('approvals.permissionKind.environment');
    case 'chat': return t('people.reference.kinds.chat');
    case 'document': return t('people.reference.kinds.document');
    case 'host':
      return t('approvals.permissionKind.host');
    default:
      return t('approvals.permissionKind.other');
  }
}

function toggled<T>(set: ReadonlySet<T>, value: T, on: boolean): Set<T> {
  const next = new Set(set);
  if (on) next.add(value);
  else next.delete(value);
  return next;
}

function AccessRow({ approval, live, onSettled }: { approval: AccessApproval; live: boolean; onSettled: (approval: Approval) => void }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName(!approval.request.membership);
  const decision = useDecision(approval.id, { onDecided: onSettled, onLost: onSettled });
  const request = approval.request;
  const asked = request.permissions;
  const models = request.options?.models ?? [];
  const efforts = request.options?.efforts ?? [];
  const [given, setGiven] = useState<ReadonlySet<string>>(() => new Set(asked));
  const [pickedModels, setPickedModels] = useState<ReadonlySet<string>>(() => new Set(models));
  const [pickedEfforts, setPickedEfforts] = useState<ReadonlySet<string>>(() => new Set(efforts));
  const [ends, setEnds] = useState<Ends>('never');
  const [customEnd,setCustomEnd]=useState('');
  const now=useNow();
  const customTime=Date.parse(customEnd);
  const validEnd=ends!=='custom'||Number.isFinite(customTime)&&customTime>now;
  const [note, setNote] = useState('');
  const [other, setOther] = useState(false);
  const may = approval.can_decide && live;
  const busy = decision.pending;
  const requesterName = 'user' in approval.requester ? approval.requester.user.display_name : approval.requester.thread.name;

  const narrowed = given.size !== asked.length;
  const optionsNarrowed = pickedModels.size !== models.length || pickedEfforts.size !== efforts.length;
  const approve = () => {
    const days = ends==='custom'?null:ENDS[ends];
    decision.decide(
      {
        approve: true,
        ...(narrowed ? { permissions: asked.filter((permission) => given.has(permission)) } : {}),
        ...(optionsNarrowed ? { options: { models: models.filter((model) => pickedModels.has(model)), efforts: efforts.filter((effort) => pickedEfforts.has(effort)) } } : {}),
        ...(ends==='custom'?{expires_at:new Date(customTime).toISOString()}:days === null ? {} : { expires_at: new Date(Date.now() + days * DAY_MS).toISOString() }),
        ...(note.trim() === '' ? {} : { note: note.trim() }),
      },
      t('approvals.approve'),
    );
  };
  const deny = () => decision.decide({ approve: false, ...(note.trim() === '' ? {} : { note: note.trim() }) }, t('approvals.deny'));

  const object = approval.object;
  const name = object !== null && object.name !== '' ? object.name : null;
  const headline = object === null || name === null ? approval.title : t('approvals.access.object', { kind: kindWord(object.kind, t), name });
  const reach = reachOf(asked.filter((permission) => given.has(permission)));
  const holder = request.holder;
  const forThread = request.for_thread?.name ?? null;

  return (
    <div data-access-row={approval.id} className="flex flex-col gap-3 py-3 first:pt-0 last:pb-0">
      <div className="flex flex-col gap-1">
        <p className="text-sm font-medium wrap-anywhere text-text first-letter:uppercase">{headline}</p>
        {forThread !== null && <p className="text-sm text-text-muted">{t('approvals.access.forThread', { thread: forThread })}</p>}
        {holder.kind === 'node' && holder.node.name !== null && <p className="text-sm text-text-muted">{t('approvals.access.holderNode', { place: holder.node.name })}</p>}
        {holder.kind === 'agents' && holder.node.name !== null && <p className="text-sm text-text-muted">{t('approvals.access.holderAgents', { place: holder.node.name })}</p>}
        {request.reason !== '' && <p className="text-sm wrap-anywhere text-text-muted">{request.reason}</p>}
      </div>

      {!other && approval.can_decide && <QuickGrant approval={approval} live={live} onSettled={onSettled} onOther={()=>setOther(true)} />}
      {other && <>{!request.membership&&<fieldset className="flex flex-col gap-1" disabled={!approval.can_decide || busy}>
        <legend className="pb-1 text-sm font-medium text-text">{t('approvals.access.gives')}</legend>
        <p className="pb-1 text-sm text-text-muted">{t('approvals.access.givesHint')}</p>
        {asked.map((permission) => (
          <Checkbox key={permission} label={permissionName(permission)} checked={given.has(permission)} disabled={!approval.can_decide || busy} onCheckedChange={(on) => setGiven((set) => toggled(set, permission, on))} />
        ))}
      </fieldset>}

      {reach.length > 0 && (
        <InlineAlert tone="warning" title={t('approvals.reach.title')}>
          <ul className="list-disc pl-6">
            {reach.map((key) => (
              <li key={key}>{t(key)}</li>
            ))}
          </ul>
        </InlineAlert>
      )}

      {models.length > 0 && (
        <fieldset className="flex flex-col gap-1" disabled={!approval.can_decide || busy}>
          <legend className="pb-1 text-sm font-medium text-text">{t('approvals.access.models')}</legend>
          {models.map((model) => (
            <Checkbox key={model} label={model} checked={pickedModels.has(model)} disabled={!approval.can_decide || busy} onCheckedChange={(on) => setPickedModels((set) => toggled(set, model, on))} />
          ))}
        </fieldset>
      )}
      {efforts.length > 0 && (
        <fieldset className="flex flex-col gap-1" disabled={!approval.can_decide || busy}>
          <legend className="pb-1 text-sm font-medium text-text">{t('approvals.access.efforts')}</legend>
          {efforts.map((effort) => (
            <Checkbox key={effort} label={effort} checked={pickedEfforts.has(effort)} disabled={!approval.can_decide || busy} onCheckedChange={(on) => setPickedEfforts((set) => toggled(set, effort, on))} />
          ))}
        </fieldset>
      )}

      {approval.can_decide && (
        <>
          <Select
            label={t('approvals.access.ends')}
            value={ends}
            onValueChange={setEnds}
            disabled={busy}
            fieldClassName="max-w-(--field-wide)"
            options={[
              { value: 'never', ...choiceOption(t, 'accessExpiry.never') },
              { value: 'day', ...choiceOption(t, 'accessExpiry.day') },
              { value: 'week', ...choiceOption(t, 'accessExpiry.week') },
              { value: 'month', ...choiceOption(t, 'accessExpiry.month') },
              {value:'custom',label:t('approvals.access.customEnd')},
            ]}
          />
          {ends==='custom'&&<Input type="datetime-local" label={t('approvals.access.endsOn')} hideLabel value={customEnd} onChange={(event)=>setCustomEnd(event.target.value)} />}
          <Textarea label={t('approvals.note')} rows={2} value={note} disabled={busy} onChange={(event) => setNote(event.target.value)} />
        </>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button action="approve" aria-label={actionName('approve', approval.title)} variant="secondary" pending={decision.pressed === t('approvals.approve')} disabled={!may || busy || !validEnd || !request.membership && given.size === 0} onClick={approve}>
          {actionName('approve', requesterName)}
        </Button>
        <Button action="deny" aria-label={actionName('deny', approval.title)} variant="danger-ghost" pending={decision.pressed === t('approvals.deny')} disabled={!may || busy} onClick={deny}>
          {actionName('deny', requesterName)}
        </Button>
        {approval.can_decide && !request.membership && given.size === 0 && <span className="text-sm text-text-muted">{t('approvals.access.nothing')}</span>}
      </div>
      {!approval.can_decide && <p className="text-sm text-text-muted">{t('approvals.cannotAnswer')}</p>}
      {approval.can_decide && !live && <p className="text-sm text-text-muted">{t('approvals.waitForLine')}</p>}
      {decision.error !== null && <ErrorAlert error={decision.error} />}
      </>}
    </div>
  );
}

/** A row that is decided: what it came to, and by whom. */
function DecidedRow({ approval }: { approval: Approval }) {
  const Icon = approval.state === 'approved' ? ShieldCheck : ShieldX;
  return (
    <p data-access-row={approval.id} data-state={approval.state} className="flex items-center gap-2 py-3 text-sm text-text-muted first:pt-0 last:pb-0">
      <Icon className={approval.state === 'approved' ? 'shrink-0 text-success' : 'shrink-0 text-text-subtle'} aria-hidden />
      <span className="flex min-w-0 flex-1 flex-wrap gap-x-3 gap-y-1 wrap-anywhere">
        <span className="font-medium text-text">{approval.title}</span><DecisionStatus approval={approval} />
      </span>
    </p>
  );
}

/**
 * The request or requests one person sent together. `settled` holds what was decided here or elsewhere
 * since the card was drawn; those rows show as decided, and the card is quiet once none is left.
 */
export function AccessCard({ items, settled, live, onSettled }: { items: readonly AccessApproval[]; settled: ReadonlyMap<string, Approval>; live: boolean; onSettled: (approval: Approval) => void }) {
  const { t } = useTranslation();
  const first = items[0];
  if (first === undefined) return null;
  const rows = items.map((item) => settled.get(item.id) ?? item);
  const waiting = rows.some((row) => row.state === 'pending');
  const who = 'user' in first.requester ? first.requester.user.display_name : first.requester.thread.name;
  return (
    <CardFrame approval={first} quiet={!waiting} title={items.length === 1 ? first.title : t('approvals.access.asksMany', { name: who, count: items.length })}>
      <div className="flex flex-col divide-y divide-border">
        {rows.map((row, index) =>
          row.state === 'pending' && row.kind === 'access' ? (
            <AccessRow key={row.id} approval={row} live={live} onSettled={onSettled} />
          ) : (
            <DecidedRow key={items[index]?.id ?? row.id} approval={row} />
          ),
        )}
      </div>
    </CardFrame>
  );
}
