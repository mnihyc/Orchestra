/** A compact identity and the acts available beside a person. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronDown } from 'lucide-react';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client as apiClient } from '@/api/client';

import { grantPerson, notifyPerson, personActionsQuery } from '@/api/personActions';
import { profileQuery } from '@/api/profile';
import { ProfileMessage } from '@/features/profile/Identity';
import type { PersonThing } from '@/features/people/personContext';
import type { AccessChoice, UserRef } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';

/** Keep the keyboard in the card after its form is replaced by the result. */
function Result({ children }: { children: ReactNode }) {
  const at = useRef<HTMLParagraphElement>(null);
  const focused = useRef(false);
  useLayoutEffect(() => { if (!focused.current) { at.current?.focus(); focused.current = true; } }, []);
  return <p ref={at} role="status" tabIndex={-1} className="text-sm text-text-muted">{children}</p>;
}

function ContextActions({ user, thing, message }: { user: UserRef; thing: PersonThing; message: ReactNode }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const actions = useQuery(personActionsQuery(thing, user.id));
  const [mode, setMode] = useState<'acts' | 'notify' | 'give' | 'sent' | 'given'>('acts');
  const [note, setNote] = useState('');
  const [notify, setNotify] = useState(false);
  const [choice, setChoice] = useState<AccessChoice>('full_view');
  const form = useRef<HTMLFormElement>(null);
  const enteredMode = useRef(mode);
  useLayoutEffect(() => {
    if (enteredMode.current === mode) return;
    enteredMode.current = mode;
    if (mode === 'notify' || mode === 'give') form.current?.querySelector<HTMLElement>('input:not([disabled]),button:not([disabled])')?.focus();
  }, [mode]);
  const noticeAttempt = useRef<{ input: string; key: string } | null>(null);
  const grantAttempt = useRef<{ input: string; key: string } | null>(null);
  const sent = useMutation({ mutationFn: () => {
    const text = actions.data?.note_chars === 0 ? '' : note;
    const input = JSON.stringify([thing, user.id, text]);
    const attempt = noticeAttempt.current;
    const key = attempt?.input === input ? attempt.key : newActKey();
    noticeAttempt.current = { input, key };
    return notifyPerson(thing, user.id, text, key);
  }, onSuccess: () => setMode('sent') });
  const selected = actions.data?.gives.includes(choice) === true ? choice : actions.data?.gives[0];
  const granted = useMutation({ mutationFn: () => {
    if (selected === undefined) return Promise.reject(new Error(t('people.quick.noAccess')));
    const input = JSON.stringify([thing, user.id, selected, notify]);
    const attempt = grantAttempt.current;
    const key = attempt?.input === input ? attempt.key : newActKey();
    grantAttempt.current = { input, key };
    return grantPerson(thing, user.id, selected, notify, key);
  }, onSuccess: () => {
    setMode('given'); void queryClient.invalidateQueries({ queryKey: personActionsQuery(thing, user.id).queryKey, exact: true });
  } });
  const begin = (next: 'notify' | 'give') => setMode(next);
  const status = actions.data?.can_view;
  return <>
    {actions.isPending ? <SkeletonRows rows={1} /> : actions.isError ? <ErrorState error={actions.error} onRetry={() => void actions.refetch()} compact /> : <>
      {status !== null && status !== undefined && <p className="text-sm text-text-muted">{t(status ? 'people.quick.canView' : 'people.quick.noAccess')}</p>}
      {mode === 'acts' && <div className="flex flex-wrap items-center gap-1">
        {actions.data.can_notify && <Button size="sm" onClick={() => begin('notify')}>{t('people.quick.notify')}</Button>}
        {actions.data.gives.length > 0 && <Button size="sm" onClick={() => begin('give')}>{t('people.quick.giveAccess')} <ChevronDown size={14} aria-hidden /></Button>}
        {message}
      </div>}
      {mode === 'notify' && <form ref={form} tabIndex={-1} className="flex flex-col gap-2" onSubmit={(event) => { event.preventDefault(); event.currentTarget.focus(); if (!sent.isPending) sent.mutate(); }}>
        <div className="flex items-center gap-1">{actions.data.note_chars === 0 ? <span className="min-w-0 flex-1 text-sm text-text-muted">{t('people.quick.notesOff')}</span> : <Input label={t('people.quick.note')} hideLabel value={note} disabled={sent.isPending} fieldClassName="min-w-0 flex-1"
          onChange={(event) => { setNote(Array.from(event.target.value).slice(0, actions.data.note_chars).join('')); sent.reset(); }} />}
          <Button type="submit" size="sm" pending={sent.isPending}>{t(sent.isPending ? 'people.quick.sending' : 'people.quick.send')}</Button></div>
        {sent.error !== null && <ErrorState title={t('people.quick.sendFailed')} error={sent.error} onRetry={() => sent.mutate()} compact />}
      </form>}
      {mode === 'give' && <form ref={form} tabIndex={-1} className="flex flex-col gap-2" onSubmit={(event) => { event.preventDefault(); event.currentTarget.focus(); if (!granted.isPending) granted.mutate(); }}>
        <div className="flex flex-wrap gap-1">{actions.data.gives.map((level) => <Button key={level} size="sm" variant={selected === level ? 'primary' : 'secondary'} aria-pressed={selected === level} disabled={granted.isPending} onClick={() => { setChoice(level); granted.reset(); }}>{t(level === 'full_view' ? 'people.quick.canView' : 'people.quick.canEdit')}</Button>)}</div>
        <div className="flex items-center justify-between gap-2"><Checkbox presentation="compact" label={t('people.quick.notifyThem')} checked={notify} disabled={granted.isPending} onCheckedChange={(next) => { setNotify(next); granted.reset(); }} />
          <Button type="submit" size="sm" pending={granted.isPending}>{t(granted.isPending ? 'people.quick.giving' : 'people.quick.giveAccess')}</Button></div>
        {granted.error !== null && <ErrorState title={t('people.quick.giveFailed')} error={granted.error} onRetry={() => granted.mutate()} compact />}
      </form>}
      {mode === 'sent' && <Result>{t('people.quick.sent')}</Result>}
      {mode === 'given' && <Result>{t('people.quick.given')}</Result>}
    </>}
  </>;
}

export function PersonQuickCard({ user, thing, identity = true }: { user: UserRef; thing?: PersonThing | undefined; identity?: boolean }) {
  const me = useStore(apiClient.session, (state) => state.userId);
  const profile = useQuery({ ...profileQuery(user.username), enabled: user.kind === 'human' && user.username !== '' && me !== user.id });
  const isSelf = me === user.id || profile.data?.is_me === true;
  const isOther = !isSelf && (me !== null || profile.data?.is_me === false);
  if (isSelf && !identity) return null;
  const message = isOther && profile.data !== undefined && profile.data.can_message ? <ProfileMessage compact profile={profile.data} /> : null;
  return <div className="flex max-w-[min(20rem,calc(100vw-2rem))] flex-col gap-2 p-3 text-text">
    {identity && <div className="flex min-w-0 items-center gap-2"><Avatar text={user.avatar.text} color={user.avatar.color} kind={user.kind === 'service' ? 'service' : 'human'} size={32} />
      <div className="min-w-0"><div className="truncate font-semibold">{user.display_name}</div>{user.username !== '' && <div className="truncate text-sm text-text-muted">@{user.username}</div>}</div></div>}
    {isOther && (thing === undefined ? message : <ContextActions user={user} thing={thing} message={message} />)}
  </div>;
}
