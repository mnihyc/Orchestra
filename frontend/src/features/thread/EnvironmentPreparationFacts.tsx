/** The native operation that currently holds environment readiness. */
import { useTranslation } from 'react-i18next';
import type { ThreadEnvironmentPreparation } from '@/generated/api-types';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
export function EnvironmentPreparationFacts({ stage }: { stage: ThreadEnvironmentPreparation | null | undefined }) {
  const { t } = useTranslation();
  if (stage === null) return null;
  return <FactDisclosure groups={[{ key: 'environment-preparation', facts: [
    { key: 'kind', label: t('placeFacts.stage'), value: stage === undefined ? null : t(`placeFacts.environmentStages.${stage.kind}`) },
    { key: 'environment', label: t('placeFacts.environment'), value: stage === undefined ? null : <ResourceName subject={{ kind: 'environment', environment: stage.environment }} /> },
    { key: 'observed', label: t('placeFacts.observed'), value: stage === undefined ? null : <RelativeTime at={stage.observed_at} /> },
  ] }]} />;
}
