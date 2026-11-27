/** How a step stands, at the end of its row: a spinner with its clock while it runs, then its time and a mark for done, failed or refused. */
import { Ban, Check, LoaderCircle, OctagonAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { ToolItem } from '@/features/timeline/titles';
import { Tooltip } from '@/ui/Tooltip';

export function StepState({ item, failed, took, recorded, cancelled }: { item: ToolItem; failed: boolean; took: string | null; recorded: boolean;
  /** What a cancelled step is called: a call the person refused, unless the step says otherwise (a sub-agent that was stopped). */
  cancelled?: string }) {
  const { t } = useTranslation();
  const ended = cancelled ?? t('timeline.step.refused');
  if (recorded && item.status === 'in_progress') return <span className="text-xs text-text-muted">{t('timeline.step.recordedRunning')}</span>;
  if (item.status === 'in_progress') {
    return (
      <span className="flex items-center gap-1 text-xs text-info">
        <LoaderCircle className="animate-spin" aria-hidden />
        {took !== null && <span data-tool-duration className="tabular-nums">{took}</span>}
        <span className="sr-only">{t('timeline.step.running')}</span>
      </span>
    );
  }
  return (
    <span className="flex items-center gap-1 text-xs text-text-subtle">
      {took !== null && <span data-tool-duration className="tabular-nums">{took}</span>}
      {item.status === 'cancelled' ? (
        <Tooltip content={ended}>
          <span role="img" aria-label={ended} className="inline-flex text-text-subtle">
            <Ban aria-hidden />
          </span>
        </Tooltip>
      ) : failed ? (
        <Tooltip content={t('timeline.step.failed')}>
          <span role="img" aria-label={t('timeline.step.failed')} className="inline-flex text-danger">
            <OctagonAlert aria-hidden />
          </span>
        </Tooltip>
      ) : (
        <span role="img" aria-label={t('timeline.step.done')} className="inline-flex text-success">
          <Check aria-hidden />
        </span>
      )}
    </span>
  );
}
