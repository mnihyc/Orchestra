/** A known session loss is said once on the sign-in form; a first visit has no such history. */
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import type { StoreApi } from 'zustand/vanilla';
import { client, type SessionState } from '@/api/client';
import { StateLine } from '@/ui/StateLine';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { useRenewalGroups } from '@/ui/RenewalFacts';

const causeWords = {
  expired: 'auth.login.sessionEnd.expired',
  revoked: 'auth.login.sessionEnd.revoked',
  account_unavailable: 'auth.login.sessionEnd.accountUnavailable',
} as const;

export function SessionEndedNotice({ session = client.session }: { session?: StoreApi<SessionState> }) {
  const { t } = useTranslation();
  const ended = useStore(session, (state) => state.status === 'signed_out' && state.sessionEnded === true);
  const fact = useStore(session, (state) => state.sessionEnd);
  const { groups } = useRenewalGroups(session);
  if (!ended) return null;
  const reason = fact === null || fact === undefined ? undefined : t(causeWords[fact.cause]);
  return <StateLine role="status" compact message={reason ?? t('auth.login.sessionEnded')}
    action={<FactDisclosure groups={[{ key: 'session-end', kind: 'recorded_outcome', facts: [
      { key: 'end', label: t('auth.login.sessionEnd.ended'), value: fact?.at_if_known ? <RelativeTime at={fact.at_if_known} /> : undefined },
      { key: 'reason', label: t('auth.login.sessionEnd.reason'), value: reason },
    ] }, ...groups]} />} className="text-text-muted" />;
}
