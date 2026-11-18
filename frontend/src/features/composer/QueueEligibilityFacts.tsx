/** Only the preflight's still-valid observation names a next queued author. */
import { useTranslation } from 'react-i18next';
import { useThreadPart } from '@/realtime/stores/thread';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';

export function QueueEligibilityFacts({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const queue = useThreadPart(threadId, (view) => view.thread?.queue);
  const observed = queue?.eligibility;
  const current = observed !== undefined && observed !== null && observed.state !== 'unreported' && observed.queue_revision === queue?.revision ? observed : null;
  const selected = current?.state === 'selected' ? current : null;
  const fragment = selected !== null ? <>{t('queueEligibilityFacts.next')} {selected.safe_author === null ? t('settings.compact.hiddenSource') : <UserChip user={selected.safe_author} size={24} />}</>
    : current?.state === 'none' && queue?.held !== true ? t('queueEligibilityFacts.none') : undefined;
  return <span data-queue-eligibility className="min-w-0">
    <FactDisclosure fragment={fragment} groups={[{ key: 'eligibility', facts: [
      { key: 'checked', label: t('queueEligibilityFacts.rechecked'), value: current === null ? undefined : <RelativeTime at={current.evaluated_at} /> },
      ...(current?.state === 'none' ? [{ key: 'none', label: t('queueEligibilityFacts.next'), value: t('queueEligibilityFacts.none') }] : [
        { key: 'author', label: t('queueEligibilityFacts.author'), value: selected === null ? undefined : selected.safe_author === null ? t('settings.compact.hiddenSource') : <UserChip user={selected.safe_author} size={24} /> },
        { key: 'authority', label: t('queueEligibilityFacts.authority'), value: selected === null ? undefined : selected.safe_authority === null ? t('settings.compact.hiddenSource') : <UserChip user={selected.safe_authority} size={24} /> },
        ...(selected?.blocking_reason === null || selected === null ? [] : [{ key: 'wait', label: t('queueEligibilityFacts.waiting'), value: t(`thread.blockedWhy.${selected.blocking_reason}`) }]),
      ]),
    ] }]} />
  </span>;
}
