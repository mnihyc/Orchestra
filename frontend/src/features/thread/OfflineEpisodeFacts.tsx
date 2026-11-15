/** One current outage uses the thread's existing status opening and its captured policy. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { SettingSource, Thread } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { useThreadPart } from '@/realtime/stores/thread';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function OfflineEpisodeFacts({ thread, fallback }: { thread: Thread; fallback: ReactNode }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const turn = useThreadPart(thread.id, (view) => thread.current_turn_id === null ? null : view.turns.get(thread.current_turn_id) ?? null);
  const recorded = turn?.offline_episode;
  const episode = thread.state === 'blocked' && thread.blocked?.reason === 'agent_offline'
    && turn !== null && turn.id === thread.current_turn_id && (turn.state === 'starting' || turn.state === 'running')
    && recorded !== undefined && recorded !== null && recorded.closed_at === null ? recorded : null;
  let fragment = fallback;
  if (episode !== null) {
    const deadline = episode.deadline;
    fragment = deadline.kind === 'indefinite' ? t('threadWaitFacts.offlineIndefinite')
      : deadline.at !== null ? <>{t('threadWaitFacts.offlineDeadline')} <RelativeTime at={deadline.at} /></>
      : Number.isSafeInteger(deadline.after_seconds)
        ? t('thread.blocked.offlineDeadlineDuration', { seconds: deadline.after_seconds, since: formatDateTime(episode.began_at, format) })
        : t('thread.blocked.offlineDeadlineBeyondCalendar', { since: formatDateTime(episode.began_at, format) });
  }
  const scope = episode?.policy_source ?? null;
  const source: SettingSource | null = scope === 'default' || scope === 'system' ? { scope, node: null }
    : scope === 'thread' ? { scope, node: { id: thread.id, kind: 'thread', name: thread.name, redacted: false } } : null;
  return <span {...(episode === null ? {} : { 'data-offline-episode': episode.episode_revision })}>
    <FactDisclosure fragment={fragment} groups={[{ key: 'outage', kind: 'recorded_outcome', facts: [
      { key: 'began', label: t('threadWaitFacts.outageBegan'), value: episode === null ? undefined : <RelativeTime at={episode.began_at} /> },
      { key: 'policy', label: t('threadWaitFacts.capturedPolicy'), value: episode === null ? undefined : t(episode.deadline.kind === 'finite' ? 'threadWaitFacts.finite' : 'threadWaitFacts.indefinite') },
      { key: 'source', label: t('threadWaitFacts.policySource'), value: scope === null ? undefined : source === null ? t('settings.compact.hiddenSource')
        : <SourceSettingLink source={source} def={{ key: 'threads.offline_deadline', class: 'parameter' }} target={{ kind: 'node', id: thread.id }} fallback={t('factDisclosure.notReported')} defaultToSystem /> },
    ] }]} />
  </span>;
}
