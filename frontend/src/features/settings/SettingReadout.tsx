/** A review uses the same setting names, units and choice words as the editable settings page. */
import { useTranslation } from 'react-i18next';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { localizedSetting } from './settingText';
import { SettingSummary } from './SettingSummary';

export function SettingReadout({ settingKey, value, from }: { settingKey: string; value: unknown; from?: { value: unknown } }) {
  const { t, i18n } = useTranslation();
  const entry = SETTINGS_REGISTRY.find((candidate) => candidate.key === settingKey);
  const def = entry === undefined ? { key: settingKey } : localizedSetting(entry, i18n);
  const title = 'title' in def ? def.title : t('settings.compact.definitionMissing');
  return <div className="flex min-w-0 flex-col gap-1">
    <span className="font-medium wrap-anywhere">{title}</span>
    {from === undefined ? <SettingSummary def={def} value={value} /> : <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
      <dt className="text-text-muted">{t('trace.before')}</dt><dd className="min-w-0"><SettingSummary def={def} value={from.value} /></dd>
      <dt className="text-text-muted">{t('trace.after')}</dt><dd className="min-w-0"><SettingSummary def={def} value={value} /></dd>
    </dl>}
  </div>;
}
