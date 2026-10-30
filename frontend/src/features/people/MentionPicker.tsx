/** Current chat readers and permitted broad targets, with count-only audiences and disabled reasons. */
import { AtSign, Bot, Users } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { Button } from '@/ui/Button';
import { MentionAgentAccess } from './MentionAgentAccess';
import { ThreadIdentity } from './ThreadIdentity';
import type { MentionChoice } from '@/generated/api-types';

export interface Candidate { key: string; name: string; display: string; count: number; available: boolean; reason: string | null; choice: MentionChoice | null }
const EVERYONE = '@all';
/** @all names current followers; role/group counts are exactly the served intersections. */
export function candidates(items: readonly MentionChoice[], followers: number, q: string, label: (choice: MentionChoice) => string = (choice) => choice.name): Candidate[] {
  const all: Candidate[] = 'all'.startsWith(q.toLowerCase()) ? [{ key: 'all', name: 'all', display: EVERYONE, count: followers, available: true, reason: null, choice: null }] : [];
  return [...all, ...items.map((choice) => ({ key: JSON.stringify(choice.target), name: choice.user?.username ?? label(choice), display: choice.user?.display_name ?? label(choice), count: choice.count, available: choice.available, reason: choice.reason, choice }))];
}
/** Only the small visible handful keeps the short button wording. */
export const BROAD_AUDIENCE = 5;
export function MentionPicker({ id, options, active, pending, error, onChoose }: { id: string; options: Candidate[]; active: number; pending: boolean; error: unknown; onChoose: (option: Candidate) => void }) {
  const { t } = useTranslation();
  const [request, setRequest] = useState<string | null>(null);
  return <div className="absolute bottom-full left-0 z-(--layer-dropdown) mb-1 max-h-[30dvh] w-(--menu-max-width) max-w-full overflow-y-auto rounded-lg border border-border bg-surface-raised p-1 shadow-md">
    {pending ? <p role="status" className="p-2 text-sm text-text-muted">{t('people.mentionLoading')}</p> : error !== null ? <ErrorAlert error={error} /> : <ul id={id} role="listbox" aria-label={t('chat.mention.label')}>
      {options.map((option, index) => <li key={option.key} id={`${id}-${index}`} role="option" aria-selected={index === active} aria-disabled={!option.available} title={option.reason ?? undefined}
        className={cx('flex flex-col gap-1 rounded-md px-2 py-1 text-sm', index === active && 'bg-accent-subtle', !option.available && 'text-text-subtle')}
        onMouseDown={(event) => { event.preventDefault(); if (option.available) onChoose(option); }}>
        <span className="flex items-center gap-2"><span aria-hidden>{option.choice?.target.kind === 'agent' ? <Bot className="size-4" /> : option.choice?.target.kind === 'role' || option.choice?.target.kind === 'group' ? <Users className="size-4" /> : <AtSign className="size-4" />}</span><span className="min-w-0 flex-1 break-words font-medium">{option.display}</span>
          {option.choice?.target.kind !== 'agent' && <span className="shrink-0 text-xs text-text-muted">{option.choice === null ? option.count === 0 ? t('people.noFollowers') : t('people.followerCount', { count: option.count }) : t('people.recipientCount', { count: option.count })}</span>}</span>
        {option.choice?.thread !== null && option.choice?.thread !== undefined && <ThreadIdentity id={option.choice.thread.id} />}
        {option.reason !== null && <span className="text-xs">{option.reason}</span>}
      </li>)}
      {options.length === 0 && <li className="p-2 text-text-muted">{t('people.noReaders')}</li>}
    </ul>}
    {options.filter((option) => !option.available && option.choice?.target.kind === 'agent').map((option) => <div key={`request:${option.key}`} className="flex flex-wrap items-center justify-between gap-2 border-t border-border px-2 py-1 text-xs"><span>{option.display}</span><Button size="sm" onClick={() => { if (option.choice?.target.kind === 'agent') setRequest(option.choice.target.thread_id); }}>{t('access.request.title')}</Button></div>)}
    {request !== null && <MentionAgentAccess id={request} onClose={() => setRequest(null)} />}
  </div>;
}
