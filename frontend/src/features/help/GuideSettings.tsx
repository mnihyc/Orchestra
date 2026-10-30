/** A decision guide keeps its related registry entries reachable after repeated questions are coalesced. */
import { useTranslation } from 'react-i18next';
import { authoredSettingGuide, settingHelpEntries } from '@/features/help/settings';
import { GUIDANCE } from '@/help/guidance';

export function GuideSettings({ article, onOpen }: { article: string; onOpen: (key: string) => void }) {
  const { t, i18n } = useTranslation();
  const keys = GUIDANCE.find((guide) => guide.id === article)?.settings ?? [];
  const entries = settingHelpEntries(i18n).filter((def) => keys.includes(def.key) || authoredSettingGuide(def.key)?.article === article);
  if (entries.length === 0) return null;
  return <section aria-label={t('helpRelease.settings')} className="mt-6 flex flex-col gap-2 border-t border-border pt-4">
    <h2 className="font-medium">{t('helpRelease.settings')}</h2>
    <ul className="flex flex-col gap-2">{entries.map((def) => <li key={def.key} className="flex flex-col gap-1">
      <button type="button" className="focus-inset self-start rounded-md text-left text-accent-text underline" onClick={() => onOpen(def.key)}>{t('helpGroups.openSetting', { setting: def.title })}</button>
      <p className="text-sm text-text-muted">{def.description}</p>
    </li>)}</ul>
  </section>;
}
