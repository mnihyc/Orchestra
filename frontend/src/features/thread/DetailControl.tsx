/** Four viewing intents use the conversation's existing depth, live-tool and diagnostics preferences. */
import { useTranslation } from 'react-i18next';
import type { ThreadViewMode, TimelinePreferences } from '@/features/timeline/preferences';
import { Select } from '@/ui/Select';

export function DetailControl({ preferences }: { preferences: Pick<TimelinePreferences, 'detail' | 'liveTools' | 'diagnostics' | 'setViewMode'> }) {
  const { t } = useTranslation();
  const mode: ThreadViewMode = preferences.detail === 'summary' ? 'focused' : preferences.diagnostics ? 'diagnostics' : preferences.liveTools ? 'live' : 'full';
  return <div data-thread-detail className="min-w-0 max-w-full">
    <Select<ThreadViewMode> label={t('thread.viewModes.label')} hideLabel value={mode} onValueChange={preferences.setViewMode} options={[
      { value: 'focused', label: t('thread.viewModes.focused'), description: t('thread.viewModes.focusedAbout') },
      { value: 'full', label: t('thread.viewModes.full'), description: t('thread.viewModes.fullAbout') },
      { value: 'live', label: t('thread.viewModes.live'), description: t('thread.viewModes.liveAbout') },
      { value: 'diagnostics', label: t('thread.viewModes.diagnostics'), description: t('thread.viewModes.diagnosticsAbout') },
    ]} />
  </div>;
}
