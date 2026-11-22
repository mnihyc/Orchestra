/** The admitted active route and current return state belong in the existing model opening. */
import { useTranslation } from 'react-i18next';
import type { CallRouteChoice, ThreadCallRoute } from '@/generated/api-types';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { modelResource } from '@/features/resource-facts/facts';
import { FactDisclosureBody } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

function Model({ choice }: { choice: CallRouteChoice }) {
  return <ResourceChip subject={modelResource({ model_name: choice.model, display_name: choice.display_name ?? '', parameters: {} }, choice.provider_id)} hint={false} />;
}

export function CallRouteFragment({ route }: { route: ThreadCallRoute }) {
  const { t } = useTranslation();
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted">{t('composer.callRoute.using')} <Model choice={route.active_override} /> · {t('composer.callRoute.failover')}</span>;
}

export function CallRouteFacts({ route }: { route: ThreadCallRoute }) {
  const { t } = useTranslation();
  const states = { when_ready: 'composer.callRoute.whenReady', stay_on_failover: 'composer.callRoute.stay', returning: 'composer.callRoute.returning', unreported: 'factDisclosure.notReported' } as const;
  return <FactDisclosureBody groups={[{ key: 'call-route', kind: 'current_setting', facts: [
    { key: 'active', label: t('composer.callRoute.active'), value: <Model choice={route.active_override} /> },
    { key: 'original', label: t('composer.callRoute.original'), value: route.original === null ? undefined : <Model choice={route.original} /> },
    { key: 'return', label: t('composer.callRoute.return'), value: t(states[route.return_state]) },
    { key: 'at', label: t('composer.callRoute.observed'), value: <RelativeTime at={route.observed_at} /> },
  ] }]} />;
}
