/** Expandable request lines reveal exact matching rows, recorded holders and individually authorized object acts. */
import { ShieldAlert } from 'lucide-react';
import { AdminModeStamp } from '@/features/access/AdminMode';
import { useViewChoice } from '@/lib/viewChoices';
import { IconButton } from '@/ui/IconButton';
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { Badge } from '@/ui/Badge';
import { Checkbox } from '@/ui/Checkbox';
import { JsonTree } from '@/ui/JsonTree';
import { UserChip } from '@/ui/UserChip';
import { ObjectActs } from './ObjectActs';
import { auditEntryAnchor } from './params';
import { recordedHolder, type RequestLine } from './requests';
import { bulkRestorable, selectionKey } from './restore';
import { auditSentence } from './sentence';
import { auditActionName, auditObjectName, auditSetting } from './words';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';

const PROFILES: readonly string[] = ['managed', 'connected'];

/** Known settings already have their new value in the sentence; details add only the previous value and lock. */
function RecordedDetails({ entry }: { entry: TraceEntry }) {
  const { t } = useTranslation();
  const setting = auditSetting(entry, t);
  if (setting !== null) return <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
    {setting.was !== null && setting.was !== setting.value && <><dt className="text-text-muted">{t('trace.previousValue')}</dt><dd className="wrap-anywhere">{setting.was}</dd></>}
    {setting.locked !== null && <><dt className="text-text-muted">{t('trace.lockState')}</dt><dd>{t(setting.locked ? 'trace.valueOn' : 'trace.valueOff')}</dd></>}
  </dl>;
  if (entry.action.startsWith('settings.') || entry.action === 'system.settings_updated') return <p className="text-text-muted">{t('trace.settingUnreported')}</p>;
  return <JsonTree value={entry.details} label={t('trace.details')} />;
}

export function TraceActor({ entry }: { entry: Pick<TraceEntry, 'actor' | 'as_admin'> }) {
  const { t } = useTranslation(); const actor = entry.actor;
  const identity = actor.kind === 'user' ? <UserChip user={actor.user} /> : <span className="text-sm wrap-anywhere">{actor.kind === 'system' ? t('trace.systemActor') : actor.for_user ? t('trace.agentFor', { thread: actor.thread.name ?? t('trace.unknown'), person: actor.for_user.display_name }) : t('trace.agentUnknown', { thread: actor.thread.name ?? t('trace.unknown') })}</span>;
  return <span className="inline-flex flex-wrap items-center gap-2">{identity}<AdminModeStamp asAdmin={entry.as_admin} /></span>;
}
export function RequestRow({ line, select, selected, onSelect, onRestore }: { line: RequestLine; select: boolean; selected: ReadonlySet<string>; onSelect: (entry: TraceEntry, checked: boolean) => void; onRestore: (entry: TraceEntry) => void }) {
  const { t } = useTranslation(); const format = useFormatContext(); const [open, setOpen] = useViewChoice<boolean>('trace', 'requestDetails', false, line.key); const entry = line.chosen; const multiple = line.rows.length > 1;
  const tick = (row: TraceEntry) => select && bulkRestorable(row) ? <Checkbox label={<span className="sr-only">{t('trace.selectRestore', { name: row.object.name ?? t('trace.unknown') })}</span>} checked={selected.has(selectionKey(row))} onCheckedChange={(checked) => onSelect(row, checked)} /> : null;
  const acts = (row: TraceEntry) => <ObjectActs object={row.object} trace {...(bulkRestorable(row) ? { onRestore: () => onRestore(row) } : {})} />;
  return <li id={auditEntryAnchor(entry.id)} className="min-w-0 px-3 py-3" data-trace-row={entry.id} data-trace-request={entry.request_id ?? undefined} data-trace-object={entry.object.id ?? undefined} onKeyDown={(event) => { if (event.key === 'Escape') { setOpen(false); event.currentTarget.querySelector<HTMLButtonElement>('[data-expand]')?.focus(); event.stopPropagation(); } }}>
    <div className="flex min-w-0 items-start gap-2">{!multiple && tick(entry)}<IconButton size="sm" data-expand action={open ? 'hide' : 'show'} aria-expanded={open} label={t(open ? 'trace.hideDetails' : 'trace.showDetails', { action: auditActionName(entry.action, t) })} onClick={() => setOpen(!open)} className="shrink-0" /><div className="flex min-w-0 flex-1 flex-col gap-1">
      <div className="flex min-h-(--control-sm) flex-wrap items-center gap-x-3 gap-y-1 [&>*]:min-h-(--control-sm) [&>*]:items-center"><time dateTime={entry.at} className="inline-flex text-sm leading-(--control-sm) text-text-muted">{formatDateTime(entry.at, format)}</time><TraceActor entry={entry} />{line.rows.some((row) => row.override) && <Badge tone="warning"><ShieldAlert className="size-4" aria-hidden />{t('trace.override')}</Badge>}</div>
      <p className="wrap-anywhere">{auditSentence(entry, t)}</p>
      {!multiple && acts(entry)}
    </div></div>
    {open && <ul className="mt-3 min-w-0 divide-y divide-border rounded-lg bg-bg-subtle px-3">{line.rows.map((row) => <li key={row.id} data-trace-detail={row.id} className="flex min-w-0 flex-col gap-2 py-3 text-sm">
      {multiple && <div className="flex items-start gap-2">{tick(row)}<div className="flex min-w-0 flex-col gap-1"><time dateTime={row.at} className="text-xs text-text-muted">{formatDateTime(row.at, format)}</time><TraceActor entry={row} /><p className="wrap-anywhere">{auditSentence(row, t)}</p></div></div>}
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1"><dt className="text-text-muted">{auditObjectName(row.object.kind, t)}</dt><dd className="wrap-anywhere">{row.object.name ?? t('trace.unknown')}</dd>{recordedHolder(row.details) && <><dt>{t('trace.holderThen')}</dt><dd className="wrap-anywhere">{recordedHolder(row.details)}</dd></>}{row.ip && <><dt>{t('trace.ip')}</dt><dd className="font-mono wrap-anywhere">{row.ip}</dd></>}</dl>
      {typeof row.details === 'object' && row.details !== null && 'profile' in row.details && typeof row.details.profile === 'string' && PROFILES.includes(row.details.profile) && <p>{t(row.details.profile === 'managed' ? 'trace.profiles.managed' : 'trace.profiles.connected')}</p>}
      <div className="min-w-0 overflow-auto"><RecordedDetails entry={row} /></div>{multiple && acts(row)}
    </li>)}</ul>}
  </li>;
}
