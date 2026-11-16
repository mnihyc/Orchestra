/** A recorded delivery decision stays with its input and never borrows the current steering policy. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { InputDeliveryResolution } from '@/generated/api-types';
import { RecordedSettingSourceLink } from '@/features/settings/RecordedSettingSourceLink';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function InputDeliveryFacts({ resolution, actual, thread }: { resolution?: InputDeliveryResolution | null; actual?: string | null; thread: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const delivery = (value: string | null | undefined): string | undefined => {
    switch (value) {
      case 'queue': case 'queued': case 'next_turn': return t('composer.deliveryFacts.queue');
      case 'steer': case 'steered': return t('composer.deliveryFacts.steer');
      case 'interrupt': case 'interrupted': return t('composer.deliveryFacts.interrupt');
      case 'deferred': return t('composer.deliveryFacts.deferred');
      default: return undefined;
    }
  };
  const fragment = resolution === undefined || resolution === null ? undefined : resolution.applied === 'queue' ? t('composer.deliveryFacts.queuedFallback')
    : resolution.applied === 'interrupt' ? t('composer.deliveryFacts.interruptedFallback') : t('composer.reason.noSteer');
  return <FactDisclosure open={open} onOpenChange={setOpen} fragment={fragment} groups={[{ key: 'delivery-resolution', kind: 'recorded_outcome', facts: [
    { key: 'requested', label: t('composer.deliveryFacts.requested'), value: delivery(resolution?.requested) },
    { key: 'applied', label: t('composer.deliveryFacts.applied'), value: delivery(resolution?.applied ?? actual) },
    { key: 'fallback', label: t('composer.deliveryFacts.source'), value: resolution === null || resolution === undefined ? undefined : resolution.source === null ? t('composer.deliveryFacts.chosen')
      : open ? <RecordedSettingSourceLink source={resolution.source} setting="broadcast.fallback" target={{ kind: 'node', id: thread }} /> : undefined },
    { key: 'at', label: t('composer.deliveryFacts.at'), value: open && resolution !== undefined && resolution !== null ? <RelativeTime at={resolution.resolved_at} /> : undefined },
  ] }]} />;
}
