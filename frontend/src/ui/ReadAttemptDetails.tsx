/** Recovery facts read the affected operation's observation; opening them performs no work. */
import { useCallback, useSyncExternalStore, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { readAttemptFacts, requestReader, subscribeReadAttempt, type ReadAttemptFacts } from '@/lib/readAttempts';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function useReadAttempt(error: unknown): ReadAttemptFacts | null {
  const reader = useStore(client.session, (state) => requestReader(state.userId, state.readerVersion));
  const subscribe = useCallback((listener: () => void) => subscribeReadAttempt(error, listener), [error]);
  const snapshot = useCallback(() => readAttemptFacts(error, reader), [error, reader]);
  return useSyncExternalStore(subscribe, snapshot, () => null);
}

export function useReadAttemptGroups(facts: ReadAttemptFacts | null): FactGroup[] {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  return facts === null ? [] : [{ key: 'read-attempt', facts: [
    { key: 'attempt', label: t('clientFacts.readAttempt'), value: formatNumber(facts.read_attempt, locale) },
    { key: 'next', label: t('clientFacts.nextAttempt'), value: facts.scheduled_retry_at === null ? undefined : <RelativeTime at={new Date(facts.scheduled_retry_at).toISOString()} /> },
  ] }];
}

/** Lets one subject combine its recorded failure and current read recovery in the same opening. */
export function useReadAttemptDisclosure(error: unknown) {
  const { t } = useTranslation();
  const facts = useReadAttempt(error), groups = useReadAttemptGroups(facts);
  const fragment = facts?.phase === 'waiting' && facts.scheduled_retry_at !== null
    ? <span>{t('clientFacts.retryEligible')} <RelativeTime at={new Date(facts.scheduled_retry_at).toISOString()} /></span> : undefined;
  return { groups, fragment };
}

export function ReadAttemptDetails({ error, fallback }: { error: unknown; fallback?: ReactNode }) {
  const { groups, fragment } = useReadAttemptDisclosure(error);
  if (groups.length === 0) return fallback ?? null;
  return <FactDisclosure groups={groups} fragment={fallback ?? fragment} />;
}
