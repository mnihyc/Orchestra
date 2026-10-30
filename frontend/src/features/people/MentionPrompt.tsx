/** An explicit choice to notify or share after a document mention is inserted. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { useLayoutEffect, useRef, type ReactNode } from 'react';
import { useStore } from 'zustand';
import { client as apiClient } from '@/api/client';
import { useTranslation } from 'react-i18next';

import { grantPerson, notifyPerson, personActionsQuery } from '@/api/personActions';
import type { PersonThing } from '@/features/people/personContext';
import type { UserRef } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';

export function MentionPrompt({ user, thing, canView, children, onClose }: { user: UserRef; thing: PersonThing; canView: boolean; children: ReactNode; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const me = useStore(apiClient.session, (state) => state.userId);
  const isSelf = me === user.id;
  useLayoutEffect(() => { if (isSelf) onClose(); }, [isSelf, onClose]);
  const attempt = useRef<{ input: string; key: string } | null>(null);
  const act = useMutation({ mutationFn: (what: 'notify' | 'view' | 'view_notify') => {
    const input = JSON.stringify([thing, user.id, what]);
    const current = attempt.current;
    const key = current?.input === input ? current.key : newActKey();
    attempt.current = { input, key };
    return what === 'notify'
      ? notifyPerson(thing, user.id, '', key) : grantPerson(thing, user.id, 'full_view', what === 'view_notify', key);
  },
  onSuccess: () => { void client.invalidateQueries({ queryKey: personActionsQuery(thing, user.id).queryKey, exact: true }); onClose(); } });
  if (isSelf) return children;
  return <Popover anchor trigger={<span className="inline-flex">{children}</span>} label={t('people.quick.notifyName', { name: user.display_name })} open onOpenAutoFocus={(event) => event.preventDefault()} onOpenChange={(open) => { if (!open && !act.isPending) onClose(); }}>
    <div className="flex max-w-[min(20rem,calc(100vw-2rem))] flex-col gap-2">
      {canView ? <div className="flex items-center gap-1"><Button size="sm" pending={act.isPending} onClick={() => act.mutate('notify')}>{act.isPending ? t('people.quick.sending') : t('people.quick.notifyName', { name: user.display_name })}</Button>
        <IconButton size="sm" variant="ghost" label={t('common.dismiss')} icon={<X aria-hidden />} disabled={act.isPending} onClick={onClose} /></div> : <>
        <p className="text-sm text-text">{t('people.quick.cannotOpen', { name: user.display_name })}</p>
        <div className="flex flex-wrap gap-1"><Button size="sm" pending={act.isPending} onClick={() => act.mutate('view')}>{t(act.isPending && act.variables === 'view' ? 'people.quick.giving' : 'people.quick.giveView')}</Button>
          <Button size="sm" pending={act.isPending} onClick={() => act.mutate('view_notify')}>{t(act.isPending && act.variables === 'view_notify' ? 'people.quick.giving' : 'people.quick.giveAndNotify')}</Button>
          <Button size="sm" variant="ghost" disabled={act.isPending} onClick={onClose}>{t('people.quick.leave')}</Button></div>
      </>}
      {act.error !== null && <ErrorState title={t(act.variables === 'notify' ? 'people.quick.sendFailed' : 'people.quick.giveFailed')} error={act.error} compact onRetry={() => act.mutate(act.variables)} />}
    </div>
  </Popover>;
}
