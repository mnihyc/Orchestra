/** One question mark opens the task directly in the existing Help surface. */
import { useTranslation } from 'react-i18next';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { TASK_GUIDES, type TaskGuideId } from '@/help/tasks';
import { IconButton } from '@/ui/IconButton';

export function TaskGuideLink({ guide, heading = null }: { guide: TaskGuideId; heading?: string | null }) {
  const { t } = useTranslation();
  const open = useHelpOpener();
  const { ref, hidden } = useGuideEntry(guide, heading, 'guide');
  const definition = TASK_GUIDES.find((one) => one.id === guide);
  if (definition === undefined) return null;
  return <span ref={ref} className="inline-flex shrink-0">{!hidden && <IconButton size="sm" action="help"
    label={t('taskGuides.open', { title: t(definition.title) })}
    data-help-guide={guide} data-help-article={guide} data-help-heading={heading ?? ''}
    onClick={() => { if (heading === null) open(guide); else open(guide, heading); }} />}</span>;
}
