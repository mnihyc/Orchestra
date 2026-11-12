/** Missing context facts remain visible without inventing a range or enabling a write. */
import { useTranslation } from 'react-i18next';
import { contextOmission, type State } from '@/features/composer/modelState';
import type { ThreadParameters } from '@/generated/api-types';
import { StateLine } from '@/ui/StateLine';

export function ContextAvailability({ parameters, states, className }: { parameters: ThreadParameters; states: readonly State[]; className?: string }) {
  const { t } = useTranslation();
  const omission = contextOmission(parameters, states);
  if (omission === null) return null;
  return <div data-context-availability={omission} className={className}>
    <StateLine compact title={t('composer.model.context.title')}
      message={t(omission === 'limits' ? 'composer.model.context.unreportedLimits' : 'composer.model.context.unreportedAdjustment')} />
  </div>;
}
