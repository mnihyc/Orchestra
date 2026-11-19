/** A known session loss is said once on the sign-in form; a first visit has no such history. */
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import type { StoreApi } from 'zustand/vanilla';
import { client, type SessionState } from '@/api/client';
import { StateLine } from '@/ui/StateLine';
import { useRenewalGroups } from '@/ui/RenewalFacts';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function SessionEndedNotice({ session = client.session }: { session?: StoreApi<SessionState> }) {
  const { t } = useTranslation();
  const ended = useStore(session, (state) => state.status === 'signed_out' && state.sessionEnded === true);
  const fact = useStore(session, (state) => state.sessionEnd);
  const { groups } = useRenewalGroups(session);
  const cause = fact === undefined || fact === null ? undefined : t(`clientFacts.sessionEnd.causes.${fact.cause}`);
  return ended ? <StateLine role="status" compact message={cause ?? t('auth.login.sessionEnded')} action={<FactDisclosure groups={[
    { key: 'session-end', kind: 'recorded_outcome', facts: [
      { key: 'reason', label: t('common.reason'), value: cause },
      { key: 'at', label: t('clientFacts.sessionEnd.ended'), value: fact?.at_if_known === undefined || fact.at_if_known === null ? undefined : <RelativeTime at={fact.at_if_known} /> },
    ] }, ...groups,
  ]} />} className="text-text-muted" /> : null;
}
