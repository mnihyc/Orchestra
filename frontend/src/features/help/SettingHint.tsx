/** The same setting help link beside an editor row or an in-place control. */
import { useTranslation } from 'react-i18next';
import { useState } from 'react';

import type { SettingsTarget } from '@/api/settings';
import { useHelpOpener } from '@/features/help/open';
import { authoredSettingGuide, settingArticle, settingDefinitionForKey, settingHelpEntries } from '@/features/help/settings';
import { useHelpState } from '@/features/help/state';
import { WhereSet } from '@/features/help/WhereSet';
import { useGuideEntry } from '@/features/help/guideEntries';
import { GUIDANCE, guidesForSetting } from '@/help/guidance';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { HelpHint } from '@/ui/HelpHint';

export function SettingHint({ def, setting, target }: { def: SettingDef; setting?: ResolvedSetting; target?: SettingsTarget; sheet?: boolean }) {
  const { t } = useTranslation();
  const open = useHelpOpener();
  const [shown, setShown] = useState(false);
  const workflow = GUIDANCE.find((guide) => guide.id === guidesForSetting(def.key)[0]);
  const authored = authoredSettingGuide(def.key);
  const related = workflow ?? (authored === null ? undefined : { id: authored.article, title: authored.title });
  const history = def.key === 'retention.agent_history_idle_days';
  const heading = history ? 'agent-history' : workflow === undefined ? (authored?.heading ?? null) : null;
  const article = history ? 'settings-and-usage' : related?.id ?? settingArticle(def.key);
  const { ref, hidden } = useGuideEntry(article, heading);
  const entry = () => {
    setShown(false);
    open(settingArticle(def.key));
    useHelpState.setState({ settingContext: target === undefined ? null : { key: def.key, target } });
  };
  return <span ref={ref} className="contents">{!hidden && <HelpHint term={def.title} what={def.description}
    open={shown} onOpenChange={setShown}
    guide={{ article, heading }}
    where={setting === undefined ? undefined : <WhereSet setting={setting} def={def} target={target} />}
    more={{ label: related === undefined ? t('helpRelease.openEntry') : t('helpGroups.readGuide', { guide: t(related.title) }), onOpen: history ? () => { setShown(false); open(article, 'agent-history'); } : related === undefined ? entry : () => {
      if (heading === null) open(related.id); else open(related.id, heading);
      useHelpState.setState({ guideContext: target === undefined ? null : { article: related.id, target } });
    } }}
    {...(related === undefined && !history ? {} : { which: <button type="button" className="text-accent-text underline" onClick={entry}>{t('helpGroups.openSetting', { setting: def.title })}</button> })} />}</span>;
}

/** In-place controls use a key; their entry is still the generated registry's, not a second hand-written hint. */
export function SettingHelpLink({ settingKey, target }: { settingKey: string; target?: SettingsTarget }) {
  const { i18n } = useTranslation();
  const def = settingDefinitionForKey(settingHelpEntries(i18n), settingKey);
  return def === undefined ? null : <SettingHint def={def} target={target} />;
}
