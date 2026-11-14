/** The three ways to read a thread: Normal folds the work of a turn into one line, Verbose draws every step, Diagnostics adds the program's own state. */
import { useTranslation } from 'react-i18next';
import type { TimelinePreferences } from '@/features/timeline/preferences';
import type { WorkDetail } from '@/features/timeline/rows';
import { Select } from '@/ui/Select';

export function DetailControl({ preferences }: { preferences: Pick<TimelinePreferences, 'detail' | 'setDetail'> }) {
  const { t } = useTranslation();
  return <div data-thread-detail className="min-w-0 max-w-full">
    <Select<WorkDetail> label={t('thread.viewModes.label')} hideLabel value={preferences.detail} onValueChange={preferences.setDetail} options={[
      { value: 'normal', label: t('thread.viewModes.normal') },
      { value: 'verbose', label: t('thread.viewModes.verbose') },
      { value: 'diagnostics', label: t('thread.viewModes.diagnostics') },
    ]} />
  </div>;
}
