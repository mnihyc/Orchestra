/** A retained delivery decision never borrows the receiver's current capabilities or defaults. */
import { useTranslation } from 'react-i18next';
import type { InputDeliveryResolution, SettingSource } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

/** `actual` is an announcement's recorded outcome: without a receipt it is the one applied fact, and nothing requested is guessed. */
export function DeliveryResolutionFacts({ resolution, threadId, actual }: { resolution: InputDeliveryResolution | null | undefined; threadId: string; actual?: string | null }) {
  const { t } = useTranslation();
  const receipt = resolution ?? null;
  const outcome = actual === 'queued' ? t('composer.queue.delivery.queue') : actual === 'steered' ? t('composer.queue.delivery.steer')
    : actual === 'interrupted' ? t('composer.queue.delivery.interrupt') : actual === 'deferred' ? t('deliveryResolutionFacts.deferred') : undefined;
  const scope = receipt?.source.kind === 'setting' ? receipt.source.scope : null;
  const source: SettingSource | null = scope === 'default' || scope === 'system' ? { scope, node: null } : null;
  return <FactDisclosure fragment={receipt === null ? undefined : t(receipt.applied === 'interrupt' ? 'deliveryResolutionFacts.interrupt' : 'deliveryResolutionFacts.queued')}
    groups={[{ key: 'delivery', kind: 'recorded_outcome', facts: [
      { key: 'requested', label: t('deliveryResolutionFacts.requested'), value: receipt === null ? undefined : t(`composer.queue.delivery.${receipt.requested}`) },
      { key: 'applied', label: t('deliveryResolutionFacts.applied'), value: receipt === null ? outcome : t(`composer.queue.delivery.${receipt.applied}`) },
      { key: 'fallback', label: t('deliveryResolutionFacts.fallback'), value: receipt === null ? undefined : <>{t(receipt.source.value === 'interrupt' ? 'composer.queue.delivery.interrupt' : 'composer.queue.delivery.queue')} · {receipt.source.kind === 'request' ? t('deliveryResolutionFacts.chosen')
        : source === null ? t('settings.compact.hiddenSource') : <SourceSettingLink source={source} def={{ key: 'broadcast.fallback', class: 'parameter' }} target={{ kind: 'node', id: threadId }} fallback={t('factDisclosure.notReported')} defaultToSystem />}</> },
      ...(receipt?.reason === 'steer_unavailable_and_reach_limited' ? [{ key: 'reason', label: t('deliveryResolutionFacts.reason'), value: t('deliveryResolutionFacts.reachLimited') }] : []),
      { key: 'resolved', label: t('deliveryResolutionFacts.resolved'), value: receipt === null ? undefined : <RelativeTime at={receipt.resolved_at} /> },
    ] }]} />;
}
