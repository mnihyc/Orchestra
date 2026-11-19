/** Installer choices are labels; round trips are measured by the target connector. */
import { useTranslation } from 'react-i18next';
import type { GatewayCatalogueEntry } from '@/generated/api-types';
import { Select } from '@/ui/Select';
import { HelpHint } from '@/ui/HelpHint';

export function GatewayChoice({ gateways, value, onChange, disabled = false }: {
  gateways: readonly GatewayCatalogueEntry[]; value: string; onChange: (value: string) => void; disabled?: boolean;
}) {
  const { t } = useTranslation();
  const missing = value !== 'automatic' && !gateways.some((gateway) => gateway.id === value);
  return <Select label={t('regionalGateways.gateway')} value={value} onValueChange={onChange} disabled={disabled}
    help={<HelpHint term={t('regionalGateways.gateway')} what={t('regionalGateways.choiceHelp')}
      guide={{ article: 'connect-a-machine', heading: 'choose-a-gateway' }} />}
    options={[
      { value: 'automatic', name: t('regionalGateways.automatic') },
      ...gateways.map((gateway) => ({ value: gateway.id, name: gateway.label, description: gateway.region === gateway.label ? undefined : gateway.region })),
      ...(missing ? [{ value, name: t('regionalGateways.unavailable'), disabled: true }] : []),
    ]} />;
}
