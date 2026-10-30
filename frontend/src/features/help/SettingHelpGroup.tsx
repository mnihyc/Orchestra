/** One question covers the settings actually present in a picker, with each full entry still reachable. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { SettingsTarget } from '@/api/settings';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { settingArticle, settingDefinitionForKey, settingHelpEntries } from '@/features/help/settings';
import { useHelpState } from '@/features/help/state';
import type { SettingDef } from '@/generated/api-types';
import { HelpHint } from '@/ui/HelpHint';

export function SettingHelpGroup({ settingKeys, target, term, article, heading = null }: {
  settingKeys: readonly string[];
  target?: SettingsTarget;
  term: string;
  article: string;
  heading?: string | null;
}) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  const show = useHelpOpener();
  const available = settingHelpEntries(i18n);
  const definitions = [...new Map(settingKeys.map((key) => settingDefinitionForKey(available, key))
    .filter((def): def is SettingDef => def !== undefined).map((def) => [def.key, def])).values()];
  const { ref, hidden } = useGuideEntry(definitions.length === 0 ? null : article, heading, 'group');
  const entry = (def: SettingDef) => {
    setOpen(false);
    show(settingArticle(def.key));
    useHelpState.setState({ settingContext: target === undefined ? null : { key: def.key, target } });
  };
  return <span ref={ref} className="contents">{!hidden && definitions.length > 0 && <HelpHint term={term}
    open={open} onOpenChange={setOpen} guide={{ article, heading }}
    what={<div className="flex flex-col gap-3"><p>{t('helpGroups.visibleSettings')}</p><dl className="flex flex-col gap-3">
      {definitions.map((def) => <div key={def.key}><dt className="font-medium">{def.title}</dt><dd className="flex flex-col gap-1">
        <span>{def.description}</span><button type="button" className="self-start text-accent-text underline" onClick={() => entry(def)}>{t('helpGroups.openSetting', { setting: def.title })}</button>
      </dd></div>)}
    </dl></div>}
    more={{ label: t('help.hint.more'), onOpen: () => {
      if (heading === null) show(article); else show(article, heading);
      useHelpState.setState({ guideContext: target === undefined ? null : { article, target } });
    } }} />}</span>;
}
