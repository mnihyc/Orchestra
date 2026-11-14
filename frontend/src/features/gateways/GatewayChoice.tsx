/** Installer choices are labels; round trips are measured by the target connector. */
import { useTranslation } from 'react-i18next';
import type { GatewayCatalogueEntry } from '@/generated/api-types';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { HelpHint } from '@/ui/HelpHint';

export function GatewayChoice({ gateways, value, onChange, disabled = false, keepCurrent = false }: {
  gateways: readonly GatewayCatalogueEntry[]; value: string; onChange: (value: string) => void; disabled?: boolean; keepCurrent?: boolean;
}) {
  const { t } = useTranslation();
  const missing = !(keepCurrent && value === '') && value !== 'automatic' && !gateways.some((gateway) => gateway.id === value);
  // UI values keep the retained choice distinct from catalogue identifiers.
  const selected = keepCurrent && value === '' ? 'keep' : value === 'automatic' ? 'automatic' : `gateway:${value}`;
  return <ResourcePicker pickerKey="gateway-choice" label={t('regionalGateways.gateway')} value={selected} onValueChange={(next) => onChange(next === 'keep' ? '' : next === 'automatic' ? next : next.slice('gateway:'.length))} disabled={disabled}
    help={<HelpHint term={t('regionalGateways.gateway')} what={t('regionalGateways.choiceHelp')}
      guide={{ article: 'connect-a-machine', heading: 'choose-a-gateway' }} />}
    options={[
      ...(keepCurrent ? [{ value: 'keep', name: t('regionalGateways.keepCurrent') }] : []),
      { value: 'automatic', name: t('regionalGateways.automatic') },
      ...gateways.map((gateway) => ({ value: `gateway:${gateway.id}`, name: gateway.label, description: gateway.region === gateway.label ? undefined : gateway.region })),
      ...(missing ? [{ value: selected, name: t('regionalGateways.unavailable'), disabled: true }] : []),
    ]} />;
}
