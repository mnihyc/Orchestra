import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { pickerGroupReport } from '@/lib/pickerDiagnostics';
import { useDeviceChoiceGroups } from '@/ui/DeviceChoiceFacts';
import { FactDisclosureBody } from '@/ui/FactDisclosure';

export function PickerDiagnostics() {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const report = useStore(pickerGroupReport);
  const { groups } = useDeviceChoiceGroups(report ?? undefined,
    report?.current === undefined ? undefined : t('clientFacts.device.loadedCollapsed', { count: report.current, number: formatNumber(report.current, locale) }),
    report?.saved === undefined ? undefined : t('clientFacts.device.savedCollapsed', { count: report.saved, number: formatNumber(report.saved, locale) }));
  if (groups.length === 0) return null;
  return <details className="mb-2">
    <summary className="cursor-pointer text-sm">{t('clientFacts.device.lastPicker')}</summary>
    <FactDisclosureBody groups={groups.map((group) => ({ ...group, title: t('clientFacts.device.collapsedGroups') }))} />
  </details>;
}
