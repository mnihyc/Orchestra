/** The current reader's confirmed catch-up choice, shared by its control and New divider. */
import { useTranslation } from 'react-i18next';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useInterfaceReading } from '@/lib/interfaceReading';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure } from '@/ui/FactDisclosure';

export function CatchupFacts() {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const reading = useInterfaceReading();
  const count = reading.personalReady ? reading.catch_up_above_items : null;
  const source = count === null ? null : reading.catch_up_above_items_own !== null
    ? <a href="/settings/interface#interface-reading" className="focus-inset rounded-md text-accent-text underline underline-offset-2">{t('settings.interface.reading.ownChoice')}</a>
    : reading.catch_up_above_items_source === null ? null : <SourceSettingLink inherited source={reading.catch_up_above_items_source}
      def={{ key: 'interface.catch_up_above_items', class: 'parameter' }} target={{ kind: 'system' }} fallback={t('settings.editor.from.system')}/>;
  return <FactDisclosure groups={[{ key: 'catch-up', kind: 'current_setting', facts: [{ key: 'automatic',
    label: count === null ? t('settingRegistry.interface.catch_up_above_items.title') : t('settings.interface.reading.automaticAfter', { count, amount: formatNumber(count, locale) }),
    value: source,
  }] }]}/>;
}
