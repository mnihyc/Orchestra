/** A driver's optional operations and their declared availability, without extra requests. */
import { useTranslation } from 'react-i18next';
import type { HostDriver } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Popover } from '@/ui/Popover';
import { StatusDot } from '@/ui/StatusDot';

export function DriverFeatures({ driver }: { driver: HostDriver }) {
  const { t } = useTranslation();
  const features = Object.entries(driver.availability ?? {});
  if (features.length === 0) return <span className="text-sm text-text-muted">–</span>;
  return <Popover label={t('hosts.driverFeatures.title')} className="w-80 max-w-[calc(100vw-2rem)]"
    trigger={<Button size="sm" variant="ghost">{t('hosts.driverFeatures.title')}</Button>}>
    <ul className="flex min-w-0 flex-col gap-2">{features.map(([feature, support]) => <li key={feature} className="flex min-w-0 flex-col gap-0.5 text-sm">
      <span className="flex items-center gap-2"><StatusDot tone={support.supported ? 'success' : 'off'} label={t(support.supported ? 'hosts.driverFeatures.available' : 'hosts.driverFeatures.unavailable')} />
        <span className="wrap-anywhere">{t(`hosts.driverFeatures.names.${feature}`, { defaultValue: feature.replaceAll('_', ' ') })}</span></span>
      <span className="pl-6 text-text-muted wrap-anywhere">{support.supported ? t('hosts.driverFeatures.available') : support.reason ?? t('hosts.driverFeatures.unavailable')}</span>
    </li>)}</ul>
  </Popover>;
}
