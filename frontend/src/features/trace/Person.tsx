/** A person's trace entry, in exactly the place the caller is looking at. */
import { useRouter } from '@tanstack/react-router';
import { ACTIONS } from '@/ui/actions';
import { useTranslation } from 'react-i18next';
import type { UserRef } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import type { MenuEntry } from '@/ui/Menu';
import { PersonCard } from '@/ui/PersonCard';
import { UserChip } from '@/ui/UserChip';
import { traceHref, type TraceScope } from './params';

export function mayReadTrail(scope: TraceScope, systemAdmin = false): boolean {
  return scope.type === 'system' ? systemAdmin : holds(scope.access, 'audit.view');
}
export function useTraceEntry(scope: TraceScope | null, allowed: boolean) {
  const { t } = useTranslation(); const router = useRouter();
  return (user: UserRef): MenuEntry[] => !allowed || scope === null ? [] : [{ id: 'trace', label: t('trace.entry'), icon: <ACTIONS.history.Icon aria-hidden />, onSelect: () => router.history.push(traceHref(scope, user.id)) }];
}
export function TracePerson({ user, scope, allowed, size = 28, showUsername = false }: { user: UserRef; scope: TraceScope; allowed: boolean; size?: 24 | 28; showUsername?: boolean }) {
  const { t } = useTranslation();
  const identity = <UserChip className="max-w-full" user={user} size={size} showUsername={showUsername} card={!allowed} />;
  return allowed ? <PersonCard user={user} actions={<Button size="sm" action="history" asChild><a href={traceHref(scope, user.id)}>{t('trace.entry')}</a></Button>}>{identity}</PersonCard> : identity;
}
