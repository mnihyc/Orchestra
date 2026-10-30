/** Where the current setting comes from, with its exact source-setting link and any controlling lock. */
import { useTranslation } from 'react-i18next';

import type { SettingsTarget } from '@/api/settings';
import { SettingLock } from '@/features/settings/SettingLock';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { emptySettingValue, SettingSummary } from '@/features/settings/SettingSummary';
import type { ResolvedSetting, SettingDef, SettingSource } from '@/generated/api-types';

export { sourceSettingHref } from '@/features/settings/sourceHref';

function SourceLink({ source, def, target }: { source: SettingSource; def: Pick<SettingDef, 'key' | 'class'>; target?: SettingsTarget }) {
  const { t } = useTranslation();
  const text = source.node?.name ?? t(source.scope === 'default' ? 'settings.editor.from.default' : source.scope === 'system' ? 'settings.editor.from.system' : 'settings.editor.from.here');
  return <SourceSettingLink source={source} def={def} target={target} text={text} fallback={text} />;
}

export function WhereSet({ setting, def, target, additionalOnly = false, values = false }: { setting: ResolvedSetting; def: Pick<SettingDef, 'key' | 'class'>; target?: SettingsTarget; additionalOnly?: boolean; values?: boolean }) {
  const { t } = useTranslation();
  const same = (left: SettingSource, right: SettingSource) => left.scope === right.scope && left.node?.id === right.node?.id;
  const parts = setting.parts.filter((part) => !part.ignored && (!additionalOnly || !same(part.source, setting.source) && (setting.locked_by === null || !same(part.source, setting.locked_by))));
  if (additionalOnly && parts.length === 0) return null;
  return <section aria-label={t('help.hint.where')} className="flex flex-col gap-2">
    {additionalOnly || parts.length > 1 ? <ul className="flex flex-col gap-2 text-sm text-text-muted">{parts.map((part, index) => <li key={`${part.source.scope}:${part.source.node?.id ?? ''}:${index}`}><SourceLink source={part.source} def={def} target={target} />
      {values && <div className="text-text">{emptySettingValue(part.value) ? t('settings.compact.none') : <SettingSummary def={def} value={part.value} />}</div>}</li>)}</ul>
      : <SourceLink source={setting.source} def={def} target={target} />}
    {!additionalOnly && setting.locked_by !== null && <SettingLock lock={setting.locked_by} def={def} />}
  </section>;
}
