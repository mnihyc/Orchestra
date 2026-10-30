/** Focused comment text with a current document-reader picker that keeps the caret in its field. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { documentReadersQuery, type DocumentMentionCandidate } from '@/api/people';
import { insertMention, mentionAt } from '@/features/chat/text';
import { MentionPrompt } from '@/features/people/MentionPrompt';
import type { UserRef } from '@/generated/api-types';
import { isComposing } from '@/lib/shortcuts';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ChoiceInspection, type CardController } from '@/ui/DetailCard';
import { ErrorState } from '@/ui/ErrorState';
import { PersonCard } from '@/ui/PersonCard';
import { Textarea } from '@/ui/Textarea';
import { UserChip } from '@/ui/UserChip';

export function CommentField({ docId, blockId, label, placeholder, value, onChange, onSend, autofocus = false }: { docId: string; blockId?: string | null | undefined; label: string; placeholder: string; value: string; onChange: (text: string, readers: readonly UserRef[]) => void; onSend: () => void; autofocus?: boolean }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const field = useRef<HTMLTextAreaElement>(null);
  const list = useId();
  const [caret, setCaret] = useState(value.length);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const [prompt, setPrompt] = useState<DocumentMentionCandidate | null>(null);
  const [extra, setExtra] = useState<{ scope: object; items: DocumentMentionCandidate[]; cursor: string | null; pending: boolean; failed: boolean; page: number } | null>(null);
  const request = useRef(0);
  const inspectors = useRef(new Map<string, CardController>());
  const mention = mentionAt(value, caret);
  const open = mention !== null && !dismissed;
  const query = mention?.query ?? '';
  const key = `${docId}\u0000${query}\u0000${open ? 'open' : 'closed'}`;
  const scope = useMemo(() => ({ key }), [key]);
  const found = useQuery({ ...documentReadersQuery(docId, query), enabled: open });
  const pages = extra?.scope === scope ? extra : null;
  const candidates = pages?.items ?? found.data?.items ?? [];
  const cursor = pages === null ? found.data?.next_cursor ?? null : pages.cursor;
  const morePending = pages?.pending === true;
  const moreFailed = pages?.failed === true;
  const count = candidates.length + (cursor === null ? 0 : 1);
  const chosen = Math.min(active, Math.max(0, count - 1));
  useEffect(() => { if (autofocus) field.current?.focus(); }, [autofocus]);
  useEffect(() => { request.current += 1; return () => { request.current += 1; }; }, [scope]);
  const more = async () => {
    if (cursor === null || morePending) return;
    const turn = ++request.current;
    setExtra({ scope, items: candidates, cursor, pending: true, failed: false, page: pages?.page ?? 0 });
    try {
      const next = await client.query(documentReadersQuery(docId, query, cursor));
      if (request.current !== turn) return;
      setExtra((previous) => previous?.scope !== scope ? previous : { scope, items: next.items, cursor: next.next_cursor, pending: false, failed: false, page: previous.page + 1 });
      setActive(0);
    } catch {
      if (request.current === turn) setExtra((previous) => previous?.scope !== scope ? previous : { ...previous, pending: false, failed: true });
    }
  };
  const pick = (candidate: DocumentMentionCandidate) => {
    if (mention === null) return;
    const user = candidate.user;
    const next = insertMention(value, mention, user.username);
    onChange(next.text, [user]); setCaret(next.caret); setDismissed(true); setPrompt(candidate);
    requestAnimationFrame(() => { field.current?.focus(); field.current?.setSelectionRange(next.caret, next.caret); });
  };
  return <div className="relative">
    {open && <div className="mb-1 max-h-48 overflow-y-auto rounded-lg border border-border bg-surface-raised p-1">
      {pages !== null && pages.page > 0 && <Button size="sm" disabled={morePending} onClick={() => { request.current += 1; setExtra(null); setActive(0); field.current?.focus(); field.current?.setSelectionRange(caret, caret); }}>{t('people.quick.firstPeople')}</Button>}
      {found.isPending ? <p role="status" className="p-2 text-sm text-text-muted">{t('common.loading')}</p> : found.isError ? <ErrorState error={found.error} onRetry={() => void found.refetch()} compact /> : <ChoiceInspection><ul id={list} role="listbox" aria-label={t('people.quick.mentionPeople')}>
        {candidates.map((candidate, index) => <PersonCard key={candidate.user.id} user={candidate.user} choice active={chosen === index} thing={{ kind: 'document', id: docId, ...(blockId === null || blockId === undefined ? {} : { anchor: { block: blockId } }) }} controller={(value) => { if (value === null) inspectors.current.delete(candidate.user.id); else inspectors.current.set(candidate.user.id, value); }} returnFocus={() => { field.current?.focus(); field.current?.setSelectionRange(caret, caret); }}><li id={`${list}-${index}`} role="option" aria-selected={chosen === index} className={cx('rounded-md p-2 text-sm', chosen === index && 'bg-accent-subtle')} onMouseDown={(event) => { event.preventDefault(); pick(candidate); }}><UserChip user={candidate.user} card={false} size={24} /> <span className="text-text-muted">@{candidate.user.username}{candidate.can_view ? '' : ` · ${t('people.quick.noAccess')}`}</span></li></PersonCard>)}
        {cursor !== null && <li id={`${list}-${candidates.length}`} role="option" aria-selected={chosen === candidates.length} aria-disabled={morePending} className={cx('rounded-md p-2 text-sm', chosen === candidates.length && 'bg-accent-subtle')} onMouseDown={(event) => { event.preventDefault(); void more(); }}>{t(morePending ? 'common.loading' : moreFailed ? 'people.quick.loadFailed' : 'people.quick.morePeople')}</li>}
        {count === 0 && <li className="p-2 text-sm text-text-muted">{t('people.noReaders')}</li>}
      </ul></ChoiceInspection>}
    </div>}
    {prompt !== null && <MentionPrompt user={prompt.user} thing={{ kind: 'document', id: docId, ...(blockId === null || blockId === undefined ? {} : { anchor: { block: blockId } }) }} canView={prompt.can_view} onClose={() => setPrompt(null)}><UserChip user={prompt.user} mention size={24} /></MentionPrompt>}
    <Textarea ref={field} label={label} hideLabel rows={2} value={value} placeholder={placeholder} aria-autocomplete="list" aria-controls={open ? list : undefined} aria-activedescendant={open && count > 0 ? `${list}-${chosen}` : undefined}
      onChange={(event) => { onChange(event.target.value, []); setCaret(event.target.selectionStart); setActive(0); setDismissed(false); }}
      onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
      onKeyDown={(event) => {
        if (isComposing(event.nativeEvent)) return;
        if (open && event.key === 'ArrowRight' && candidates[chosen] !== undefined) { event.preventDefault(); inspectors.current.get(candidates[chosen].user.id)?.keep(); return; }
        if (open && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) { event.preventDefault(); if (count > 0) setActive((chosen + (event.key === 'ArrowDown' ? 1 : count - 1)) % count); return; }
        if (open && (event.key === 'Enter' || event.key === 'Tab') && candidates[chosen] !== undefined) { event.preventDefault(); pick(candidates[chosen]); return; }
        if (open && (event.key === 'Enter' || event.key === 'Tab') && cursor !== null && chosen === candidates.length) { event.preventDefault(); void more(); return; }
        if (open && event.key === 'Escape') { event.preventDefault(); setDismissed(true); return; }
        if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); onSend(); }
      }} />
  </div>;
}
