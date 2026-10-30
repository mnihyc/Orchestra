/** The import's current step stays clear on both a wide screen and a phone. */
import { useTranslation } from 'react-i18next';

import { useCompactControls } from '@/features/workbench/Narrow';
import { formatNumber } from '@/lib/format';
import { useAppearance } from '@/lib/appearance';
import { cx } from '@/ui/cx';

/** The views of one import, in their order. */
export type ImportStep = 'source' | 'destination' | 'review' | 'run' | 'report';

const STEPS: readonly ImportStep[] = ['source', 'destination', 'review', 'run', 'report'];

export function ImportSteps({ at, onBack, locked = false }: { at: ImportStep; onBack: (step: ImportStep) => void; locked?: boolean }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const compact = useCompactControls();
  const index = STEPS.indexOf(at);
  if (compact) return <p aria-label={t('importWizard.steps.label')} aria-current="step" className="text-sm font-medium text-text">
    {t('importWizard.steps.compact', { number: formatNumber(index + 1, locale), total: formatNumber(STEPS.length, locale), step: t(`importWizard.steps.${at}`) })}
  </p>;
  return <ol aria-label={t('importWizard.steps.label')} className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
    {STEPS.map((step, position) => {
      const current = step === at;
      const words = `${formatNumber(position + 1, locale)} ${t(`importWizard.steps.${step}`)}`;
      const mayReturn = !locked && position < index && index < STEPS.indexOf('run');
      return <li key={step} aria-current={current ? 'step' : undefined} className="flex items-center gap-2">
        {mayReturn ? <button type="button" className="focus-inset rounded-sm hover:text-text hover:underline" onClick={() => onBack(step)}>{words}</button>
          : <span className={cx(current && 'font-medium text-text')}>{words}</span>}
        {position < STEPS.length - 1 && <span aria-hidden>·</span>}
      </li>;
    })}
  </ol>;
}
