/** Only this browser's actual renewal observation is shown, without polling or exposing credentials. */
import { useStore } from 'zustand';
import type { StoreApi } from 'zustand/vanilla';
import { useTranslation } from 'react-i18next';
import { client, type SessionState } from '@/api/client';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function useRenewalGroups(session: StoreApi<SessionState>) {
  const { t } = useTranslation();
  const fact = useStore(session, (state) => state.renewal);
  const groups: FactGroup[] = fact === undefined || fact === null ? [] : [{ key: 'renewal', facts: [
    { key: 'phase', label: t('clientFacts.renewal'), value: t(`clientFacts.renewalPhase.${fact.phase}`) },
    { key: 'next', label: t('clientFacts.nextAttempt'), value: fact.scheduled_at === null ? undefined : <RelativeTime at={new Date(fact.scheduled_at).toISOString()} /> },
    { key: 'renewed', label: t('clientFacts.renewed'), value: fact.renewed_at === null ? undefined : <RelativeTime at={new Date(fact.renewed_at).toISOString()} /> },
  ] }];
  return { fact, groups };
}

export function RenewalFacts({ session = client.session, quiet = false }: { session?: StoreApi<SessionState>; quiet?: boolean }) {
  const { t } = useTranslation();
  const { fact, groups } = useRenewalGroups(session);
  return <FactDisclosure groups={groups} fragment={!quiet && fact?.phase === 'failed' ? t('clientFacts.renewalFailed') : undefined} />;
}
