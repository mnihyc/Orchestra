/** The Performance view's shared frame, with the same scope, span and filters as Usage. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { Measure } from '@/features/usage/search';
import { Select } from '@/ui/Select';
import { PerformanceFigures } from '@/features/usage/Performance';
import { boxClasses, Section } from '@/shell/Page';

export interface PerformanceViewProps {
  measure: Extract<Measure, 'speed' | 'first_token' | 'errors'>;
  onMeasure: (measure: Extract<Measure, 'speed' | 'first_token' | 'errors'>) => void;
  figures?: ReactNode;
  chart?: ReactNode;
  comparison?: ReactNode;
  breakdowns?: ReactNode;
  controls?: ReactNode;
  errors?:ReactNode;
}

export function PerformanceView({ measure, onMeasure, figures, chart, comparison, breakdowns, controls,errors }: PerformanceViewProps) {
  const { t } = useTranslation();
  return <section aria-label={t('usagePerf.performanceView')} className="flex min-w-0 flex-col gap-4">
    {figures ?? <PerformanceFigures current={undefined} />}
    <Section id="performance-over-time" title={t('usage.sections.overTime')} className={`${boxClasses} min-w-0 p-4`} action={<><Select size="sm" label={t('usage.chart.show')} value={measure} onValueChange={onMeasure} options={PERFORMANCE_MEASURES.map((value) => ({ value, name: t(`usagePerf.choices.measures.${value}.name`), description: t(`usagePerf.choices.measures.${value}.description`) }))} />{controls}</>}>
      {chart ?? <div className="flex min-h-40 items-center justify-center text-sm text-text-muted">{t('usagePerf.notMeasured')}</div>}
    </Section>
    {comparison ?? <Section id="usage-models" title={t('usagePerf.sections.models')}><p className="text-sm text-text-muted">{t('usagePerf.notMeasured')}</p></Section>}
    {errors}
    {breakdowns}
  </section>;
}

const PERFORMANCE_MEASURES = ['speed', 'first_token', 'errors'] as const;
